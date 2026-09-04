use std::error::Error;

use kithara_bufpool::PoolError;
use kithara_encode::EncodeError;
use kithara_worker::TaskError;
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

/// Terminal live-recorder failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LiveRecordingError {
    /// Live master output is stereo, but the selected profile is not.
    #[error("live recording requires 2 channels, got {0}")]
    ChannelCount(u16),
    /// A configured frame or sample capacity cannot be represented.
    #[error("live recording capacity overflow")]
    CapacityOverflow,
    /// The configured source sample rate is zero.
    #[error("live recording sample rate must be > 0")]
    InvalidSampleRate,
    /// The composition-owned sample pool rejected recorder scratch.
    #[error(transparent)]
    Pool(#[from] PoolError),
    /// The bounded RT-to-worker PCM ring ran out of room.
    #[error("live recording PCM buffer of {buffer_frames} frames overflowed")]
    BufferOverflow {
        /// Configured ring capacity.
        buffer_frames: usize,
    },
    /// The bounded format-generation queue ran out of room.
    #[error("live recording format queue of {capacity} generations overflowed")]
    GenerationQueueOverflow {
        /// Configured generation queue capacity.
        capacity: usize,
    },
    /// The destination could not open a new part.
    #[error("live recording failed to open part {part}: {source}")]
    OpenPart {
        /// One-based part sequence number.
        part: u64,
        /// Destination acquisition failure.
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    /// Encoding or publication of one part failed.
    #[error("live recording part {part} failed: {source}")]
    Part {
        /// One-based part sequence number.
        part: u64,
        /// Encoding, container, or destination failure.
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    /// Total live frame count cannot be represented.
    #[error("live recording frame count overflow")]
    FrameCountOverflow,
    /// The recorder task was cancelled before a clean finish.
    #[error("live recording cancelled")]
    Cancelled,
    /// Worker admission or lifecycle failure.
    #[error(transparent)]
    Worker(#[from] TaskError),
}
