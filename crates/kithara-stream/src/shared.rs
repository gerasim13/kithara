use std::{
    io::{self, SeekFrom},
    ops::Range,
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
    inner: Arc<Mutex<Stream<T>>>,
}

impl<T: StreamType> SharedStream<T> {
    #[must_use]
    pub fn new(stream: Stream<T>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(stream)),
        }
    }

    /// Off-real-time read from the session's own byte position: parks
    /// event-driven until the range resolves.
    ///
    /// Reached only through a [`CursorReader`](crate::CursorReader) opened
    /// with [`WaitMode::Block`](crate::WaitMode::Block) — whether a read waits
    /// is the reading session's property, never the stream's, so two sessions
    /// over one stream can answer it differently.
    ///
    /// # Errors
    ///
    /// Propagates the inner [`Stream`]'s blocking read adapter.
    pub fn blocking_read_from(&self, from: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.lock().blocking_read_from(from, buf)
    }

    /// Off-real-time seek from the session's own byte position: primes the
    /// target range instead of doing position math alone. The counterpart to
    /// [`Self::blocking_read_from`] — a session that waits for its bytes waits
    /// for them at a seek too.
    ///
    /// # Errors
    ///
    /// Propagates the inner [`Stream`]'s blocking seek adapter.
    pub fn blocking_seek_from(&self, from: u64, pos: SeekFrom) -> io::Result<u64> {
        self.inner.lock().blocking_seek_from(from, pos)
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
            pub fn probe_read_from(&self, from: u64, buf: &mut [u8]) -> io::Result<usize>;
            /// Credit `n` consumed bytes to the stream's own position — the
            /// track's published frontier. Only the published reading session
            /// calls this; a session preparing a decoder nobody hears keeps
            /// its consumption to itself.
            pub fn advance(&self, n: u64);
            /// Real-time seek resolved against an explicit position, without
            /// moving the stream's cursor. See [`Stream::probe_seek_from`].
            ///
            /// # Errors
            ///
            /// Propagates [`Stream::probe_seek_from`].
            pub fn probe_seek_from(&self, from: u64, pos: SeekFrom) -> io::Result<u64>;
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
        }
    }
}
