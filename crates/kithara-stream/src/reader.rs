use std::io::{Read, Seek};

use kithara_platform::sync::Arc;

use crate::{BoxedEventSink, ByteMap, MediaInfo, VariantTransition};

/// Move-only byte input owned by one decoder session.
pub trait SessionReader: Read + Seek + Send + Sync + 'static {}

impl<T> SessionReader for T where T: Read + Seek + Send + Sync + 'static {}

/// Reader capability and byte-stream facts captured when opening a decoder.
#[non_exhaustive]
pub struct OpenedReader {
    byte_len: Option<u64>,
    byte_map: Option<Arc<dyn ByteMap>>,
    event_sink: Option<BoxedEventSink>,
    input: Box<dyn SessionReader>,
}

impl OpenedReader {
    /// Bundle a reader with the facts resolved by the byte-stream owner.
    #[must_use]
    pub fn new<R: SessionReader>(
        input: R,
        byte_len: Option<u64>,
        byte_map: Option<Arc<dyn ByteMap>>,
        event_sink: Option<BoxedEventSink>,
    ) -> Self {
        Self {
            byte_len,
            byte_map,
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

    /// Transfer reader-side observation to the decoder.
    pub fn take_event_sink(&mut self) -> Option<BoxedEventSink> {
        self.event_sink.take()
    }

    /// Transfer byte input to the decoder.
    #[must_use]
    pub fn into_inner(self) -> Box<dyn SessionReader> {
        self.input
    }
}

/// Target facts needed to choose a decoder and its reader requirements before
/// an incoming session is opened.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct VariantReaderPlan {
    base_offset: u64,
    media_info: MediaInfo,
    transition: VariantTransition,
}

impl VariantReaderPlan {
    /// Bind target media facts and the reader coordinate origin to one exact
    /// transition.
    #[must_use]
    pub const fn new(
        transition: VariantTransition,
        media_info: MediaInfo,
        base_offset: u64,
    ) -> Self {
        Self {
            base_offset,
            media_info,
            transition,
        }
    }

    /// Exact transition that owns the planned reader.
    #[must_use]
    pub const fn transition(&self) -> VariantTransition {
        self.transition
    }

    /// Media facts used to select the decoder and reader profile.
    #[must_use]
    pub const fn media_info(&self) -> &MediaInfo {
        &self.media_info
    }

    /// Coordinate origin of the reader passed to the decoder.
    #[must_use]
    pub const fn base_offset(&self) -> u64 {
        self.base_offset
    }
}

/// Move-only incoming reader bound to one exact variant reader plan.
#[non_exhaustive]
pub struct OpenedVariantReader {
    plan: VariantReaderPlan,
    reader: OpenedReader,
}

impl OpenedVariantReader {
    /// Bind byte capabilities to the exact facts used to prepare the session.
    #[must_use]
    pub fn new(plan: VariantReaderPlan, reader: OpenedReader) -> Self {
        Self { plan, reader }
    }

    /// Exact pre-open plan used to construct this reader.
    #[must_use]
    pub const fn plan(&self) -> &VariantReaderPlan {
        &self.plan
    }

    /// Target media facts captured with the reader.
    #[must_use]
    pub const fn media_info(&self) -> &MediaInfo {
        self.plan.media_info()
    }

    /// Exact transition that owns this reader.
    #[must_use]
    pub const fn transition(&self) -> VariantTransition {
        self.plan.transition()
    }

    /// Coordinate origin of the reader passed to the decoder.
    #[must_use]
    pub const fn base_offset(&self) -> u64 {
        self.plan.base_offset()
    }

    /// Split the move-only bundle for decoder construction.
    #[must_use]
    pub fn split(self) -> (VariantReaderPlan, OpenedReader) {
        (self.plan, self.reader)
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
