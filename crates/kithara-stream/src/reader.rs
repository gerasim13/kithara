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

/// Move-only incoming reader bound to one exact variant transition.
#[non_exhaustive]
pub struct OpenedVariantReader {
    media_info: MediaInfo,
    reader: OpenedReader,
    transition: VariantTransition,
}

impl OpenedVariantReader {
    /// Bind target media facts and byte capabilities to one transition.
    #[must_use]
    pub fn new(transition: VariantTransition, media_info: MediaInfo, reader: OpenedReader) -> Self {
        Self {
            media_info,
            reader,
            transition,
        }
    }

    /// Target media facts captured with the reader.
    #[must_use]
    pub const fn media_info(&self) -> &MediaInfo {
        &self.media_info
    }

    /// Exact transition that owns this reader.
    #[must_use]
    pub const fn transition(&self) -> VariantTransition {
        self.transition
    }

    /// Split the move-only bundle for decoder construction.
    #[must_use]
    pub fn split(self) -> (VariantTransition, MediaInfo, OpenedReader) {
        (self.transition, self.media_info, self.reader)
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
