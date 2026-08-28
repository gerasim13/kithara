use kithara_bufpool::SampleBuffer;
use thiserror::Error;

#[cfg(feature = "beat-nn")]
#[path = "backend.rs"]
pub(super) mod backend;

/// Raw detector output: beat / downbeat positions in seconds from track
/// start.
#[derive(Debug)]
pub(crate) struct RawBeats {
    pub(crate) beats: SampleBuffer,
    pub(crate) downbeats: SampleBuffer,
}

/// Failure of a beat detector backend.
#[derive(Debug, Error)]
pub(crate) enum BeatDetectError {
    #[error("beat analysis buffer budget exhausted")]
    Buffer,
    /// Only the `beat-nn` factory constructs this; gated with it.
    #[cfg(feature = "beat-nn")]
    #[error("beat detector init failed: {reason}")]
    Init { reason: String },
    /// Detection can only fail when a detector backend runs (`beat-nn`) or a
    /// test scripts a failure; without either it is unconstructable.
    #[cfg(any(test, feature = "beat-nn"))]
    #[error("beat detection failed: {reason}")]
    Detect { reason: String },
}

/// Swappable beat/downbeat detector over one mono analysis window.
#[cfg_attr(test, kithara_test_macros::mock(api = [BeatDetectorMock]))]
pub(crate) trait BeatDetector: Send {
    /// # Errors
    /// [`BeatDetectError::Detect`] when the backend fails on this input.
    fn detect(&mut self, mono_window: &[f32]) -> Result<RawBeats, BeatDetectError>;
}
