use std::error::Error;

use kithara_encode::EncodeError;
use thiserror::Error;

/// Failure while encoding or publishing one recording part.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RecordingError<E: Error + 'static> {
    /// Encoding or container failure.
    #[error(transparent)]
    Encode(#[from] EncodeError),
    /// Destination write or commit failure.
    #[error("recording sink failed: {0}")]
    Sink(#[source] E),
    /// Operation attempted after the transaction stopped being active.
    #[error("recording part is no longer active")]
    Inactive,
    /// Total input frame count cannot be represented.
    #[error("recording frame count overflow")]
    FrameCountOverflow,
    /// A finite recording ended at a different frame count than requested.
    #[error("recording expected {expected} frames but received {actual}")]
    FrameCountMismatch {
        /// Requested complete frame count.
        expected: u64,
        /// Complete frame count received by the core.
        actual: u64,
    },
}

/// Result produced by a recording core using sink error `E`.
pub type RecordingResult<T, E> = Result<T, RecordingError<E>>;
