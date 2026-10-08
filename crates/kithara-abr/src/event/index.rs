/// A validated position into a peer's variant list.
///
/// Keeps a variant position from being confused with a byte offset, a segment index, or
/// an index into a different variant list. Construct via [`VariantIndex::try_new`] at
/// trust boundaries (FFI, UI), or [`VariantIndex::new`] when validity is already
/// structurally guaranteed (atomic reload, playlist parse).
#[derive(Clone, Copy, Debug, derive_more::Display, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct VariantIndex(usize);

impl VariantIndex {
    /// Wrap an index whose validity is already guaranteed by construction
    /// (atomic reload, playlist id, test literal). No bounds check.
    #[must_use]
    pub const fn new(idx: usize) -> Self {
        Self(idx)
    }

    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }

    /// Validated constructor: `Ok` iff `idx < available`.
    ///
    /// # Errors
    /// Returns [`BoundsError`] when `idx >= available`.
    pub const fn try_new(idx: usize, available: usize) -> Result<Self, BoundsError> {
        if idx < available {
            Ok(Self(idx))
        } else {
            Err(BoundsError {
                available,
                requested: idx,
            })
        }
    }
}

/// A variant index out of range against a known variant count.
#[derive(Clone, Copy, Debug, derive_more::Display, PartialEq, Eq)]
#[display("variant index {requested} out of bounds (available: {available})")]
#[derive(derive_more::Error)]
#[error(ignore)]
pub struct BoundsError {
    pub available: usize,
    pub requested: usize,
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{BoundsError, VariantIndex};

    #[kithara::test]
    fn variant_index_try_new_accepts_in_range() {
        assert_eq!(VariantIndex::try_new(0, 3), Ok(VariantIndex::new(0)));
        assert_eq!(VariantIndex::try_new(2, 3), Ok(VariantIndex::new(2)));
    }

    #[kithara::test]
    #[case(3, 3)]
    #[case(4, 3)]
    #[case(usize::MAX, 3)]
    fn variant_index_try_new_rejects_out_of_range(#[case] idx: usize, #[case] available: usize) {
        assert_eq!(
            VariantIndex::try_new(idx, available),
            Err(BoundsError {
                requested: idx,
                available,
            })
        );
    }

    #[kithara::test]
    fn variant_index_try_new_against_empty_list_always_fails() {
        assert!(VariantIndex::try_new(0, 0).is_err());
    }

    #[kithara::test]
    fn variant_index_get_round_trips() {
        assert_eq!(VariantIndex::new(7).get(), 7);
    }

    #[kithara::test]
    fn variant_index_display_is_the_bare_index() {
        assert_eq!(format!("{}", VariantIndex::new(42)), "42");
    }
}
