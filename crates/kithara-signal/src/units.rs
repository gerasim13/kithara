/// A count of audio frames, with one sample per channel in each frame.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct FrameCount(usize);

/// A count of interleaved samples.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct SampleCount(usize);

impl FrameCount {
    #[must_use]
    pub const fn new(frames: usize) -> Self {
        Self(frames)
    }

    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

impl SampleCount {
    #[must_use]
    pub const fn new(samples: usize) -> Self {
        Self(samples)
    }

    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}
