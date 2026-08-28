/// One bounded terminal-drain step from an [`crate::ElasticEngine`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ElasticDrain {
    /// Interleaved PCM frames written by this step.
    frames: usize,
    /// Whether this step released all source retained by the backend.
    complete: bool,
}

impl ElasticDrain {
    /// Construct one terminal-drain step for an engine implementation.
    #[must_use]
    pub const fn new(frames: usize, complete: bool) -> Self {
        Self { frames, complete }
    }

    /// Interleaved PCM frames written by this step.
    #[must_use]
    pub const fn frames(self) -> usize {
        self.frames
    }

    /// Whether this step released all source retained by the backend.
    #[must_use]
    pub const fn complete(self) -> bool {
        self.complete
    }
}
