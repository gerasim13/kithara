use std::io::{Read, Seek};

use kithara_platform::sync::Arc;

use crate::{BoxedEventSink, ByteMap};

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
