use kithara_beat::BeatDetectError;
use kithara_bufpool::PoolError;
use thiserror::Error;

/// Why the beat pass could not turn audio into marks: either the detector
/// refused the window, or the pass could not bring the audio to the rate the
/// detector reads. The detector contract itself belongs to `kithara-beat`.
#[derive(Debug, Error)]
pub(crate) enum BeatPassError {
    #[error(transparent)]
    Detector(#[from] BeatDetectError),
    #[error("beat analysis resampler failed: {reason}")]
    Resample { reason: String },
}

impl From<PoolError> for BeatPassError {
    fn from(error: PoolError) -> Self {
        Self::Detector(BeatDetectError::Buffer(error))
    }
}
