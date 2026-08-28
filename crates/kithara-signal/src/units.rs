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

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn count_units_preserve_their_values() {
        assert_eq!(FrameCount::new(128).get(), 128);
        assert_eq!(SampleCount::new(256).get(), 256);
    }
}
