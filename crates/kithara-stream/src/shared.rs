use std::{
    io::{self, Read, Seek, SeekFrom},
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};

use delegate::delegate;
use kithara_platform::sync::{Arc, Mutex};

use crate::{
    Activity, ByteMap, MediaInfo, PlayheadWrite, SeekControl, SeekObserve, SourcePhase,
    SourceSeekAnchor, Stream, StreamType, WorkerWake,
};

/// Shared handle to one [`Stream`], cloneable and `Send + Sync`.
///
/// A `Stream` owns its source and reads through `&mut self`, so anything that
/// reads it has to hold it exclusively for the call. This is the one place
/// that exclusivity is arranged, so every consumer — the decoder reading
/// bytes, the audio FSM asking about phase, the seek engine resolving an
/// anchor — works from the same stream without any of them owning it.
pub struct SharedStream<T: StreamType> {
    /// Construction-phase read mode, shared across clones. When set, `Read`
    /// routes through the blocking off-RT [`Stream::read`] adapter (waits for
    /// the seeked range to download, cancel-bounded by its own timeout)
    /// instead of the non-blocking RT [`Stream::probe_read`]. `Audio::new`
    /// arms it for the single up-front decoder build (with the init body
    /// prefetched, the build read normally hits committed bytes; the blocking
    /// adapter is the bounded safety net for residual lateness) and disarms it
    /// before the worker is registered, so the RT decode loop the worker then
    /// drives always uses `probe_read`.
    blocking: Arc<AtomicBool>,
    inner: Arc<Mutex<Stream<T>>>,
}

impl<T: StreamType> SharedStream<T> {
    #[must_use]
    pub fn new(stream: Stream<T>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(stream)),
            blocking: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Off-real-time read: parks event-driven until the range resolves.
    ///
    /// # Errors
    ///
    /// Propagates the inner [`Stream`]'s blocking read adapter.
    pub fn blocking_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        Read::read(&mut *self.inner.lock(), buf)
    }

    /// Arm/disarm the construction-phase blocking read mode (see the
    /// `blocking` field). Shared across all clones derived from this handle,
    /// so arming before the decoder build and disarming before worker
    /// registration is a staged construction → steady-state ownership
    /// transfer — never a live toggle once the RT worker runs.
    pub fn set_blocking(&self, on: bool) {
        self.blocking.store(on, Ordering::Release);
    }

    delegate! {
        to self.inner.lock() {
            #[must_use]
            pub fn position(&self) -> u64;
            /// Absolute byte cursor set — forwards to the inner source's
            /// atomic, used post-seek when the audio FSM lands at a
            /// known byte position.
            pub fn set_position(&self, pos: u64);
            #[must_use]
            pub fn len(&self) -> Option<u64>;
            #[must_use]
            pub fn is_empty(&self) -> Option<bool>;
            #[must_use]
            pub fn media_info(&self) -> Option<MediaInfo>;
            #[must_use]
            pub fn abr_handle(&self) -> Option<kithara_abr::AbrHandle>;
            /// Header byte range for a decoder recreate after a format change.
            ///
            /// # Errors
            ///
            /// Propagates [`Stream::format_change_segment_range`]: not
            /// applicable to sources without a variant control.
            pub fn format_change_segment_range(&self) -> crate::StreamResult<Range<u64>>;
            pub fn clear_variant_fence(&self);
            #[must_use]
            pub fn has_variant_change_pending(&self) -> bool;
            #[must_use]
            pub fn variant_change_target(&self) -> Option<usize>;
            /// Resolve a deterministic time-based seek anchor.
            ///
            /// # Errors
            ///
            /// Propagates [`Stream::seek_time_anchor`]: the source failed to
            /// resolve the anchor.
            pub fn seek_time_anchor(&self, position: kithara_platform::time::Duration) -> Result<Option<SourceSeekAnchor>, io::Error>;
            /// Build a fresh reader-side event-sink instance from the inner source.
            #[must_use]
            pub fn take_reader_event_sink(&self) -> Option<crate::BoxedEventSink>;
            /// Pull a clone of the optional byte-map handle from the
            /// inner source. Used by the decoder factory to activate the
            /// segment-by-segment fMP4 path on HLS.
            #[must_use]
            pub fn byte_map(&self) -> Option<Arc<dyn ByteMap>>;
            /// Narrow mutating playhead handle.
            #[must_use]
            pub fn playhead_write(&self) -> Arc<dyn PlayheadWrite>;
            /// Narrow seek-control handle.
            #[must_use]
            pub fn seek_control(&self) -> Arc<dyn SeekControl>;
            /// Narrow seek-observe handle.
            #[must_use]
            pub fn seek_observe(&self) -> Arc<dyn SeekObserve>;
            /// Narrow activity handle.
            #[must_use]
            pub fn activity(&self) -> Arc<dyn Activity>;
            /// Overall source readiness at current position.
            #[must_use]
            pub fn phase(&self) -> SourcePhase;
            /// Point-in-time readiness for a specific byte range.
            #[must_use]
            pub fn phase_at(&self, range: Range<u64>) -> SourcePhase;
            /// The reader→peer wake handle — `Some` for segmented sources
            /// (HLS) that push a downloader peer. The FSM arms it on the
            /// produce core (seek-apply / finalize); the scheduler shell
            /// flushes it off the forbid-blocking path.
            #[must_use]
            pub fn peer_wake(&self) -> Option<Arc<crate::DeferredWake>>;
            /// Install the audio worker's data-arrival wake on the inner
            /// source. Segmented sources (HLS) fire it from their off-RT
            /// write/settle sites; no-op for non-segmented sources. Set once,
            /// after the worker exists.
            pub fn set_worker_wake(&self, wake: Arc<dyn WorkerWake>);
            /// Real-time read: a not-ready range surfaces immediately instead
            /// of parking, so the caller can re-tick. See [`Stream::probe_read`].
            ///
            /// # Errors
            ///
            /// Propagates [`Stream::probe_read`]: `Interrupted` for transient
            /// backpressure, `Other` at a variant boundary, the source's own
            /// error otherwise.
            pub fn probe_read(&self, buf: &mut [u8]) -> io::Result<usize>;
            /// Real-time on-core seek (FSM recreate/boundary, decoder reader):
            /// position math + cursor set, no `prime_seek_range` spin on the
            /// forbid-blocking produce core. See [`Stream::probe_seek`].
            ///
            /// # Errors
            ///
            /// Propagates [`Stream::probe_seek`]: the target is unresolvable
            /// or lies past a known end.
            pub fn probe_seek(&self, pos: SeekFrom) -> io::Result<u64>;
        }
    }
}

impl<T: StreamType> Clone for SharedStream<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            blocking: Arc::clone(&self.blocking),
        }
    }
}

impl<T: StreamType> Read for SharedStream<T> {
    /// Steady state (RT worker / decoder reader): the non-blocking
    /// [`Stream::probe_read`] — a not-ready range surfaces immediately so the
    /// scheduler parks and re-ticks. During construction only (the `blocking`
    /// flag), routes through the blocking off-RT [`Stream::read`] adapter so
    /// the single up-front decoder build waits for residual init lateness
    /// instead of erroring on the first not-ready probe.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut stream = self.inner.lock();
        if self.blocking.load(Ordering::Acquire) {
            stream.read(buf)
        } else {
            stream.probe_read(buf)
        }
    }
}

impl<T: StreamType> Seek for SharedStream<T> {
    delegate! {
        to self.inner.lock() {
            fn seek(&mut self, pos: SeekFrom) -> io::Result<u64>;
        }
    }
}
