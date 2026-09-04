use std::error::Error;

/// Transactional byte destination for one independently playable part.
///
/// An uncommitted sink must discard its pending transaction when dropped.
pub trait RecordingSink: Send + 'static {
    /// Readable or published value produced by commit.
    type Output;
    /// Storage or transport failure.
    type Error: Error + Send + Sync + 'static;

    /// Write bytes at an absolute container offset.
    ///
    /// # Errors
    /// Returns the destination's write failure.
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<(), Self::Error>;

    /// Atomically publish the completed part at its exact length.
    ///
    /// # Errors
    /// Returns the destination's commit failure.
    fn commit(&mut self, final_len: u64) -> Result<Self::Output, Self::Error>;

    /// Discard the open transaction. Calling this more than once is harmless.
    fn abort(&mut self);
}

/// Opens the transactional destination for each independently playable part.
pub trait PartSinkFactory: Send + 'static {
    /// Concrete transaction used by the recording core.
    type Sink: RecordingSink;

    /// Open one part by its one-based sequence number.
    ///
    /// # Errors
    /// Returns the destination's acquisition failure.
    fn open(&mut self, part: u64) -> Result<Self::Sink, <Self::Sink as RecordingSink>::Error>;
}
