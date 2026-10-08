use std::marker::PhantomData;

/// A count tagged with its unit, without trait bounds on the unit marker.
#[derive_where::derive_where(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Count<Unit>(
    usize,
    #[derive_where(skip(Debug))] PhantomData<fn() -> Unit>,
);

/// The unit marker for audio frames.
pub enum Frames {}

/// The unit marker for interleaved samples.
pub enum Samples {}

/// A count of audio frames, with one sample per channel in each frame.
///
/// Frame and sample counts are distinct types.
///
/// ```compile_fail
/// use kithara_signal::{FrameCount, SampleCount};
/// let frames: FrameCount = SampleCount::new(1);
/// ```
pub type FrameCount = Count<Frames>;

/// A count of interleaved samples.
pub type SampleCount = Count<Samples>;

impl<Unit> Count<Unit> {
    /// Creates a count in the tagged unit.
    #[must_use]
    pub const fn new(count: usize) -> Self {
        Self(count, PhantomData)
    }

    /// Returns the number of units.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{Count, FrameCount, SampleCount};

    #[kithara::test]
    fn counts_keep_value_traits_without_marker_bounds() {
        enum Unit {}

        fn assert_traits<Value: Clone + Copy + std::fmt::Debug + Default + Eq + Ord>() {}
        const COUNT: Count<Unit> = Count::new(3);
        const VALUE: usize = COUNT.get();

        assert_traits::<Count<Unit>>();
        assert_traits::<FrameCount>();
        assert_traits::<SampleCount>();

        assert_eq!(VALUE, 3);
        assert_eq!(COUNT, Count::new(3));
        assert_eq!(Count::<Unit>::default().get(), 0);
        assert_eq!(format!("{COUNT:?}"), "Count(3)");
        assert!(Count::<Unit>::default() < COUNT);
        assert_eq!(
            COUNT.partial_cmp(&Count::new(4)),
            Some(std::cmp::Ordering::Less)
        );
        assert_eq!(size_of::<Count<Unit>>(), size_of::<usize>());
    }
}
