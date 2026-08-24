use std::{
    io::{self, Read, Seek, SeekFrom},
    ops::Range,
};

use delegate::delegate;
use kithara_platform::sync::{Arc, Mutex};
use kithara_stream::{
    Activity, ByteMap, ConstructionGate, MediaInfo, OpenedReader, PlayheadWrite, SeekControl,
    SeekObserve, SourcePhase, SourceSeekAnchor, Stream, StreamResult, StreamType, WaitOutcome,
    WorkerWake,
};

use super::offset::OffsetReader;

/// Shared stream wrapper for format change detection.
///
/// Wraps Stream in `Arc<Mutex>` to allow:
/// - Decoder to read via Read + Seek
/// - `StreamAudioSource` to check `media_info()` for format changes
pub(crate) struct SharedStream<T: StreamType> {
    inner: Arc<Mutex<Stream<T>>>,
    /// Construction mode for one decoder reader. Coordinator clones carry no
    /// gate; every opened reader receives a fresh gate so an off-RT rebuild
    /// cannot switch the active decoder to blocking I/O.
    construction_gate: Option<ConstructionGate>,
}

impl<T: StreamType> SharedStream<T> {
    pub(crate) fn new(stream: Stream<T>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(stream)),
            construction_gate: None,
        }
    }

    pub(crate) fn open_initial_reader(&self) -> OpenedReader {
        let construction_gate = ConstructionGate::default();
        OpenedReader::new(
            self.with_construction_gate(construction_gate.clone()),
            self.len(),
            self.byte_map(),
            Some(construction_gate),
            self.take_reader_event_sink(),
        )
    }

    pub(crate) fn open_rebuild_reader(&self, base_offset: u64) -> OpenedReader {
        let byte_len = self.len().map(|length| length.saturating_sub(base_offset));
        let byte_map = self.byte_map();
        let event_sink = self.take_reader_event_sink();
        let construction_gate = ConstructionGate::default();
        let input = OffsetReader::new(
            self.with_construction_gate(construction_gate.clone()),
            base_offset,
        );
        OpenedReader::new(
            input,
            byte_len,
            byte_map,
            Some(construction_gate),
            event_sink,
        )
    }

    fn with_construction_gate(&self, construction_gate: ConstructionGate) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            construction_gate: Some(construction_gate),
        }
    }

    delegate! {
        to self.inner.lock() {
            pub(crate) fn position(&self) -> u64;
            /// Absolute byte cursor set — forwards to the inner source's
            /// atomic, used post-seek when the audio FSM lands at a
            /// known byte position.
            pub(crate) fn set_position(&self, pos: u64);
            pub(crate) fn len(&self) -> Option<u64>;
            pub(crate) fn media_info(&self) -> Option<MediaInfo>;
            pub(crate) fn abr_handle(&self) -> Option<kithara_abr::AbrHandle>;
            pub(crate) fn format_change_segment_range(&self) -> StreamResult<Range<u64>>;
            pub(crate) fn seek_time_anchor(&self, position: kithara_platform::time::Duration) -> Result<Option<SourceSeekAnchor>, io::Error>;
            /// Build a fresh reader-side event-sink instance from the inner source.
            pub(crate) fn take_reader_event_sink(&self) -> Option<kithara_stream::BoxedEventSink>;
            /// Pull a clone of the optional byte-map handle from the
            /// inner source. Used by the decoder factory to activate the
            /// segment-by-segment fMP4 path on HLS.
            pub(crate) fn byte_map(&self) -> Option<Arc<dyn ByteMap>>;
            pub(crate) fn seek_prepare(&self) -> Option<Arc<dyn kithara_stream::SeekPrepare>>;
            /// Narrow mutating playhead handle.
            pub(crate) fn playhead_write(&self) -> Arc<dyn PlayheadWrite>;
            /// Narrow seek-control handle.
            pub(crate) fn seek_control(&self) -> Arc<dyn SeekControl>;
            /// Narrow seek-observe handle.
            pub(crate) fn seek_observe(&self) -> Arc<dyn SeekObserve>;
            /// Narrow activity handle.
            pub(crate) fn activity(&self) -> Arc<dyn Activity>;
            /// Overall source readiness at current position.
            pub(crate) fn phase(&self) -> SourcePhase;
            /// Point-in-time readiness for a specific byte range.
            pub(crate) fn phase_at(&self, range: Range<u64>) -> SourcePhase;
            /// Zero-budget readiness probe that also files `range` as reader
            /// demand with the source — the channel dispatch budgets follow.
            /// See [`Stream::probe_wait`].
            pub(crate) fn probe_wait(&self, range: Range<u64>) -> StreamResult<WaitOutcome>;
            /// The reader→peer wake handle — `Some` for segmented sources
            /// (HLS) that push a downloader peer. The FSM arms it on the
            /// produce core (seek-apply / finalize); the scheduler shell
            /// flushes it off the forbid-blocking path.
            pub(crate) fn peer_wake(&self) -> Option<Arc<kithara_stream::DeferredWake>>;
            /// Install the audio worker's data-arrival wake on the inner
            /// source. Segmented sources (HLS) fire it from their off-RT
            /// write/settle sites; no-op for non-segmented sources. Set once,
            /// after the worker exists.
            pub(crate) fn set_worker_wake(&self, wake: Arc<dyn WorkerWake>);
            /// Real-time on-core seek (FSM recreate/boundary, decoder
            /// `OffsetReader`): position math + cursor set, no `prime_seek_range`
            /// spin on the forbid-blocking produce core. See [`Stream::probe_seek`].
            pub(crate) fn probe_seek(&self, pos: SeekFrom) -> io::Result<u64>;
        }
    }
}

impl<T: StreamType> Clone for SharedStream<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            construction_gate: self.construction_gate.clone(),
        }
    }
}

impl<T: StreamType> Read for SharedStream<T> {
    /// Steady state (RT worker / `OffsetReader`): the non-blocking
    /// [`Stream::probe_read`] — a not-ready range surfaces immediately so the
    /// scheduler parks and re-ticks. During this reader's construction only,
    /// routes through the blocking off-RT [`Stream::read`] adapter so
    /// construction waits for residual init lateness instead of erroring on
    /// the first not-ready probe.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut stream = self.inner.lock();
        if self
            .construction_gate
            .as_ref()
            .is_some_and(ConstructionGate::is_armed)
        {
            stream.read(buf)
        } else {
            stream.probe_read(buf)
        }
    }
}

impl<T: StreamType> Seek for SharedStream<T> {
    delegate! {
        to self.inner.lock() {
            /// Always the blocking [`Stream::seek`]. The construction gate
            /// picks the read mode, not the seek mode: a decoder seeks past
            /// residual lateness in steady state too, and a probe seek there
            /// answers not-ready to a caller that can only ask again. Staying
            /// off the blocking path is `OffsetReader`'s own choice, made by
            /// naming `probe_seek`.
            fn seek(&mut self, pos: SeekFrom) -> io::Result<u64>;
        }
    }
}
