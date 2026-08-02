use std::{
    io::{Read, Seek},
    sync::atomic::{AtomicBool, Ordering},
};

use kithara_platform::{sync::Arc, time::Duration};

use crate::{BoxedEventSink, ByteMap, MediaInfo, VariantTransition};

/// Shared switch for blocking reads during off-RT decoder construction.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct ConstructionGate {
    armed: Arc<AtomicBool>,
}

impl ConstructionGate {
    /// Enable blocking construction reads.
    pub fn arm(&self) {
        self.armed.store(true, Ordering::Release);
    }

    /// Restore non-blocking steady-state reads.
    pub fn disarm(&self) {
        self.armed.store(false, Ordering::Release);
    }

    /// Whether construction reads should wait for source bytes.
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::Acquire)
    }
}

/// Move-only byte input owned by one decoder session.
pub trait SessionReader: Read + Seek + Send + Sync + 'static {}

impl<T> SessionReader for T where T: Read + Seek + Send + Sync + 'static {}

/// Reader capability and byte-stream facts captured when opening a decoder.
#[non_exhaustive]
pub struct OpenedReader {
    input: Box<dyn SessionReader>,
    byte_len: Option<u64>,
    byte_map: Option<Arc<dyn ByteMap>>,
    construction_gate: Option<ConstructionGate>,
    event_sink: Option<BoxedEventSink>,
}

impl OpenedReader {
    /// Bundle a reader with the facts resolved by the byte-stream owner.
    #[must_use]
    pub fn new<R: SessionReader>(
        input: R,
        byte_len: Option<u64>,
        byte_map: Option<Arc<dyn ByteMap>>,
        construction_gate: Option<ConstructionGate>,
        event_sink: Option<BoxedEventSink>,
    ) -> Self {
        Self {
            byte_len,
            byte_map,
            construction_gate,
            event_sink,
            input: Box::new(input),
        }
    }

    /// Bytes addressable by the captured reader view.
    #[must_use]
    pub const fn byte_len(&self) -> Option<u64> {
        self.byte_len
    }

    /// Segment map captured by the byte-stream owner.
    #[must_use]
    pub fn byte_map(&self) -> Option<Arc<dyn ByteMap>> {
        self.byte_map.clone()
    }

    /// Construction gate supplied by the byte-stream owner.
    #[must_use]
    pub fn construction_gate(&self) -> Option<ConstructionGate> {
        self.construction_gate.clone()
    }

    /// Transfer byte input to the decoder.
    #[must_use]
    pub fn into_inner(self) -> Box<dyn SessionReader> {
        self.input
    }

    /// Transfer reader-side observation to the decoder.
    pub fn take_event_sink(&mut self) -> Option<BoxedEventSink> {
        self.event_sink.take()
    }
}

/// Target facts needed to choose a decoder and its reader requirements before
/// an incoming session is opened.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct VariantReaderPlan {
    landing_time: Duration,
    media_info: MediaInfo,
    transition: VariantTransition,
}

impl VariantReaderPlan {
    /// Bind target media facts and landing time to one exact transition.
    #[must_use]
    pub const fn new(
        transition: VariantTransition,
        media_info: MediaInfo,
        landing_time: Duration,
    ) -> Self {
        Self {
            landing_time,
            media_info,
            transition,
        }
    }

    /// Content time where the incoming decoder must land.
    #[must_use]
    pub const fn landing_time(&self) -> Duration {
        self.landing_time
    }

    /// Media facts used to select the decoder and reader profile.
    #[must_use]
    pub const fn media_info(&self) -> &MediaInfo {
        &self.media_info
    }

    /// Exact transition that owns the planned reader.
    #[must_use]
    pub const fn transition(&self) -> VariantTransition {
        self.transition
    }
}

/// Move-only incoming reader bound to one exact variant reader plan.
#[non_exhaustive]
pub struct OpenedVariantReader {
    reader: OpenedReader,
    plan: VariantReaderPlan,
}

impl OpenedVariantReader {
    /// Bind byte capabilities to the exact facts used to prepare the session.
    #[must_use]
    pub fn new(plan: VariantReaderPlan, reader: OpenedReader) -> Self {
        Self { reader, plan }
    }

    /// Content time where the incoming decoder must land.
    #[must_use]
    pub const fn landing_time(&self) -> Duration {
        self.plan.landing_time()
    }

    /// Target media facts captured with the reader.
    #[must_use]
    pub const fn media_info(&self) -> &MediaInfo {
        self.plan.media_info()
    }

    /// Exact pre-open plan used to construct this reader.
    #[must_use]
    pub const fn plan(&self) -> &VariantReaderPlan {
        &self.plan
    }

    /// Split the move-only bundle for decoder construction.
    #[must_use]
    pub fn split(self) -> (VariantReaderPlan, OpenedReader) {
        (self.plan, self.reader)
    }

    /// Exact transition that owns this reader.
    #[must_use]
    pub const fn transition(&self) -> VariantTransition {
        self.plan.transition()
    }
}

/// Result of taking the reader owned by one exact variant transition.
#[non_exhaustive]
pub enum VariantReaderTake {
    /// The transition is live, but its construction bytes are not ready yet.
    Preparing,
    /// The move-only reader is ready and transferred to the caller.
    Ready(OpenedVariantReader),
    /// The exact transition is live, but its reader was already transferred.
    Taken,
    /// The transition was superseded, aborted, promoted, or never existed.
    Stale,
}
