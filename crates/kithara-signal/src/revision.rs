use std::{fmt, marker::PhantomData, num::NonZeroU64};

/// A non-zero monotonic revision, without trait bounds on its domain marker.
///
/// Revisions from different domains are distinct types.
///
/// ```compile_fail
/// use kithara_signal::Revision;
/// enum Transport {}
/// enum Topology {}
/// let transport: Revision<Transport> = Revision::<Topology>::first();
/// ```
#[derive_where::derive_where(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Revision<Tag>(
    NonZeroU64,
    #[derive_where(skip(Debug))] PhantomData<fn() -> Tag>,
);

impl<Tag> Revision<Tag> {
    /// Returns the next owner-assigned revision, or `None` on exhaustion.
    #[must_use]
    pub fn checked_next(self) -> Option<Self> {
        self.0
            .get()
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .map(Self::from)
    }

    /// Returns the first owner-assigned revision.
    #[must_use]
    pub const fn first() -> Self {
        Self(NonZeroU64::MIN, PhantomData)
    }
}

impl<Tag> fmt::Display for Revision<Tag> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl<Tag> From<NonZeroU64> for Revision<Tag> {
    fn from(value: NonZeroU64) -> Self {
        Self(value, PhantomData)
    }
}

impl<Tag> From<Revision<Tag>> for u64 {
    fn from(revision: Revision<Tag>) -> Self {
        revision.0.get()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fmt::{Debug, Display},
        hash::{DefaultHasher, Hash, Hasher},
        num::NonZeroU64,
    };

    use kithara_test_utils::kithara;

    use super::Revision;

    enum Tag {}

    #[kithara::test]
    fn revisions_keep_value_traits_without_marker_bounds() {
        fn assert_traits<
            Value: Clone + Copy + Debug + Display + Eq + Hash + Ord + Into<u64> + From<NonZeroU64>,
        >() {
        }
        const FIRST: Revision<Tag> = Revision::first();

        assert_traits::<Revision<Tag>>();

        let first = Revision::<Tag>::from(NonZeroU64::MIN);
        assert_eq!(FIRST, first);
        assert_eq!(u64::from(first), 1);
        assert_eq!(first.to_string(), "1");
        assert_eq!(format!("{first:?}"), "Revision(1)");

        let mut revision_hasher = DefaultHasher::new();
        let mut value_hasher = DefaultHasher::new();
        first.hash(&mut revision_hasher);
        NonZeroU64::MIN.hash(&mut value_hasher);
        assert_eq!(revision_hasher.finish(), value_hasher.finish());
        assert_eq!(size_of::<Revision<Tag>>(), size_of::<NonZeroU64>());
        assert_eq!(size_of::<Option<Revision<Tag>>>(), size_of::<NonZeroU64>());
    }

    #[kithara::test]
    fn revisions_advance_without_wrapping() {
        let first = Revision::<Tag>::first();
        let Some(next) = first.checked_next() else {
            panic!("the first revision has a successor");
        };
        assert_eq!(u64::from(next), 2);
        assert!(first < next);
        assert_eq!(first.partial_cmp(&next), Some(std::cmp::Ordering::Less));
        assert_eq!(Revision::<Tag>::from(NonZeroU64::MAX).checked_next(), None);
    }
}
