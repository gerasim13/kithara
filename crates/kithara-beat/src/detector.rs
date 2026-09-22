use kithara_bufpool::PoolError;
use thiserror::Error;

use crate::RawBeats;

/// Why a detector could not produce marks for a window.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BeatDetectError {
    /// The detector could not take the buffers it needs from the pool.
    #[error("beat analysis buffer allocation failed: {0}")]
    Buffer(#[from] PoolError),
    /// The detector could not be built.
    #[error("beat detector init failed: {reason}")]
    Init {
        /// What the backend reported.
        reason: String,
    },
    /// The detector ran and failed.
    #[error("beat detection failed: {reason}")]
    Detect {
        /// What the backend reported.
        reason: String,
    },
}

/// A beat detector: mono audio at the detector's own rate in, marks out.
///
/// Both backends in this crate implement it, and so does anything a caller
/// supplies of its own; the analysis pass drives one through this contract and
/// never names a backend.
#[cfg_attr(
    any(test, feature = "mock"),
    kithara_test_macros::mock(api = [BeatDetectorMock])
)]
pub trait BeatDetector: Send + Sync {
    /// Detect beats and downbeats in one window of mono audio.
    ///
    /// # Errors
    ///
    /// Returns [`BeatDetectError`] when the detector cannot take its buffers
    /// or the backend fails.
    fn detect(&self, mono_window: &[f32]) -> Result<RawBeats, BeatDetectError>;
}
