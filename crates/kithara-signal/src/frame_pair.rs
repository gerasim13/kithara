use std::marker::PhantomData;

#[derive_where::derive_where(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FramePair<Tag> {
    first: usize,
    second: usize,
    #[derive_where(skip(Debug))]
    marker: PhantomData<fn() -> Tag>,
}

impl<Tag> FramePair<Tag> {
    #[must_use]
    pub const fn new(first: usize, second: usize) -> Self {
        Self {
            first,
            second,
            marker: PhantomData,
        }
    }

    #[must_use]
    pub const fn first(self) -> usize {
        self.first
    }

    #[must_use]
    pub const fn second(self) -> usize {
        self.second
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::FramePair;

    #[kithara::test]
    fn pairs_preserve_order_without_marker_bounds() {
        enum Tag {}

        fn assert_traits<Value: Clone + Copy + std::fmt::Debug + Eq>() {}

        assert_traits::<FramePair<Tag>>();
        const PAIR: FramePair<Tag> = FramePair::new(2, 7);
        assert_eq!((PAIR.first(), PAIR.second()), (2, 7));
        assert_eq!(PAIR, FramePair::new(2, 7));
        assert_eq!(size_of::<FramePair<Tag>>(), size_of::<[usize; 2]>());
    }
}
