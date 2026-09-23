use std::{ffi::c_void, ptr::NonNull};

use super::{
    format::OwnedFormat,
    sys,
    sys::{Off64, SSize},
};
use crate::error::AndroidBackendError;

/// Byte source behind an `AMediaDataSource`. The platform calls it serially.
pub trait MediaDataSource: Send + 'static {
    /// Read into `buf` starting at `offset`, returning the byte count, or
    /// `None` when the source failed. A count of zero reports end of input.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Option<usize>;

    /// Total length of the source, or `None` when it has none to state.
    fn size(&self) -> Option<u64>;
}

/// Container parsing through `AMediaExtractor`, fed by a caller-supplied
/// [`MediaDataSource`].
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get_mut)]
pub struct OwnedExtractor<S> {
    #[field(get_mut)]
    source: Box<S>,
    data_source: NonNull<sys::AMediaDataSource>,
    raw: NonNull<sys::AMediaExtractor>,
    track_count: usize,
}

// SAFETY: the wrapper exclusively owns both NDK handles and its pinned callback
// context; operations require a mutable borrow and NDK serializes callbacks.
unsafe impl<S: Send> Send for OwnedExtractor<S> {}

impl<S: MediaDataSource> OwnedExtractor<S> {
    /// Step to the next sample. Blocks on a streaming source, which can leave
    /// the source's own failure record set.
    pub fn advance(&mut self) {
        // SAFETY: the extractor is live; its previous sample has been copied out.
        unsafe { sys::AMediaExtractor_advance(self.raw.as_ptr()) };
    }

    /// Open an extractor over `source`.
    ///
    /// # Errors
    ///
    /// Returns the source back with the platform's refusal, so a caller whose
    /// source recorded its own failure can report that instead.
    pub fn open(source: S) -> Result<Self, (S, AndroidBackendError)> {
        let mut source = Box::new(source);

        // SAFETY: the constructor takes no arguments; its nullable result is checked.
        let Some(ds) = NonNull::new(unsafe { sys::AMediaDataSource_new() }) else {
            return Err((
                *source,
                AndroidBackendError::operation("extractor-data-source-new", "returned null"),
            ));
        };

        // SAFETY: ds is live; the boxed source stays at this address until after
        // the extractor and data source are deleted.
        unsafe {
            sys::AMediaDataSource_setUserdata(ds.as_ptr(), (&raw mut *source).cast::<c_void>());
            sys::AMediaDataSource_setReadAt(ds.as_ptr(), Some(read_at_thunk::<S>));
            sys::AMediaDataSource_setGetSize(ds.as_ptr(), Some(get_size_thunk::<S>));
        }

        // SAFETY: `AMediaExtractor_new` returns NULL on failure.
        let Some(raw) = NonNull::new(unsafe { sys::AMediaExtractor_new() }) else {
            // SAFETY: `ds` is live and unreferenced; nothing took ownership of it.
            unsafe { sys::AMediaDataSource_delete(ds.as_ptr()) };
            return Err((
                *source,
                AndroidBackendError::operation("extractor-new", "returned null"),
            ));
        };

        // SAFETY: both handles are live, and the data source outlives the extractor.
        let status = unsafe { sys::AMediaExtractor_setDataSourceCustom(raw.as_ptr(), ds.as_ptr()) };
        if status != sys::MEDIA_STATUS_OK {
            // SAFETY: both handles are owned here and no wrapper will free them on this exit.
            unsafe {
                sys::AMediaExtractor_delete(raw.as_ptr());
                sys::AMediaDataSource_delete(ds.as_ptr());
            }
            return Err((
                *source,
                AndroidBackendError::status("AMediaExtractor_setDataSourceCustom", status),
            ));
        }

        // SAFETY: extractor is live.
        let track_count = unsafe { sys::AMediaExtractor_getTrackCount(raw.as_ptr()) };

        Ok(Self {
            source,
            raw,
            track_count,
            data_source: ds,
        })
    }

    /// Fetch the next sample's bytes. `None` reports that the platform
    /// returned no sample, which is end of input unless the source recorded a
    /// failure of its own.
    pub fn read_sample(&mut self, buf: &mut [u8]) -> Option<usize> {
        // SAFETY: the extractor is live and buf is writable for its declared length.
        let read = unsafe {
            sys::AMediaExtractor_readSampleData(self.raw.as_ptr(), buf.as_mut_ptr(), buf.len())
        };
        usize::try_from(read).ok()
    }

    /// Presentation time of the current sample in microseconds, or -1 when
    /// the extractor holds no sample.
    #[must_use]
    pub fn sample_time_us(&self) -> i64 {
        // SAFETY: extractor is live.
        unsafe { sys::AMediaExtractor_getSampleTime(self.raw.as_ptr()) }
    }

    /// Seek to the nearest sync sample at or before `pts_us`.
    ///
    /// # Errors
    ///
    /// Returns [`AndroidBackendError::Status`] when the platform refuses the
    /// seek.
    pub fn seek_to_previous_sync(&mut self, pts_us: i64) -> Result<(), AndroidBackendError> {
        // SAFETY: extractor is live.
        let status = unsafe {
            sys::AMediaExtractor_seekTo(self.raw.as_ptr(), pts_us, sys::SEEK_MODE_PREVIOUS_SYNC)
        };
        if status != sys::MEDIA_STATUS_OK {
            return Err(AndroidBackendError::status(
                "AMediaExtractor_seekTo",
                status,
            ));
        }
        Ok(())
    }

    /// Select the track the following reads belong to.
    ///
    /// # Errors
    ///
    /// Returns [`AndroidBackendError::Status`] when the platform refuses the
    /// selection.
    pub fn select_track(&mut self, index: usize) -> Result<(), AndroidBackendError> {
        // SAFETY: the caller bounds `index` by `track_count`.
        let status = unsafe { sys::AMediaExtractor_selectTrack(self.raw.as_ptr(), index) };
        if status != sys::MEDIA_STATUS_OK {
            return Err(AndroidBackendError::status(
                "AMediaExtractor_selectTrack",
                status,
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn track_count(&self) -> usize {
        self.track_count
    }

    /// Format of one track.
    ///
    /// # Errors
    ///
    /// Returns [`AndroidBackendError::Operation`] when the platform has no
    /// format for `index`.
    pub fn track_format(&self, index: usize) -> Result<OwnedFormat, AndroidBackendError> {
        // SAFETY: the extractor is live; the returned format is independently owned.
        let raw = unsafe { sys::AMediaExtractor_getTrackFormat(self.raw.as_ptr(), index) };
        NonNull::new(raw).map(OwnedFormat::from).ok_or_else(|| {
            AndroidBackendError::operation("extractor-track-format", "returned null")
        })
    }
}

impl<S> Drop for OwnedExtractor<S> {
    fn drop(&mut self) {
        // SAFETY: both handles are uniquely owned; the callback context is still alive.
        unsafe {
            sys::AMediaExtractor_delete(self.raw.as_ptr());
            sys::AMediaDataSource_delete(self.data_source.as_ptr());
        }
    }
}

extern "C" fn read_at_thunk<S: MediaDataSource>(
    userdata: *mut c_void,
    offset: Off64,
    buffer: *mut c_void,
    size: usize,
) -> SSize {
    // SAFETY: userdata names the pinned source retained by OwnedExtractor. The
    // NDK invokes this callback serially with a writable buffer of size bytes.
    let (source, slice) = unsafe {
        (
            &mut *userdata.cast::<S>(),
            std::slice::from_raw_parts_mut(buffer.cast::<u8>(), size),
        )
    };

    let Ok(offset) = u64::try_from(offset) else {
        return -1;
    };
    source
        .read_at(offset, slice)
        .and_then(|read| SSize::try_from(read).ok())
        .unwrap_or(-1)
}

extern "C" fn get_size_thunk<S: MediaDataSource>(userdata: *mut c_void) -> Off64 {
    /// `AMediaDataSourceGetSize` reads -1 as "the source has no known length".
    const SIZE_UNKNOWN: i64 = -1;

    // SAFETY: userdata names the pinned source retained until both NDK handles drop.
    let source = unsafe { &*userdata.cast::<S>() };
    source
        .size()
        .and_then(|size| Off64::try_from(size).ok())
        .unwrap_or(SIZE_UNKNOWN)
}
