use std::num::NonZeroU64;

use portable_atomic::{AtomicU64, Ordering};

/// Stable identity of one beat-grid owner.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    derive_more::Display,
    derive_more::Into,
)]
#[display("{_0}")]
#[into(u64)]
#[repr(transparent)]
pub struct BeatGridId(NonZeroU64);

impl BeatGridId {
    /// Allocates an identity unique to this process.
    ///
    /// Every grid owner uses this allocation site so points from independent
    /// sessions and tracks cannot acquire equal stamps accidentally.
    ///
    /// # Errors
    ///
    /// Returns [`BeatGridIdAllocationError`] after the non-zero identity space
    /// has been exhausted.
    pub fn allocate() -> Result<Self, BeatGridIdAllocationError> {
        static NEXT: AtomicU64 = AtomicU64::new(1);

        let value = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                (current != 0).then(|| current.wrapping_add(1))
            })
            .map_err(|_| BeatGridIdAllocationError)?;
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(BeatGridIdAllocationError)
    }
}

/// The process-wide beat-grid identity space is exhausted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("beat grid identity space is exhausted")]
pub struct BeatGridIdAllocationError;

/// Monotonic revision of one [`BeatGridId`].
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    derive_more::Display,
    derive_more::Into,
)]
#[display("{_0}")]
#[into(u64)]
#[repr(transparent)]
pub struct BeatGridRevision(NonZeroU64);

impl BeatGridRevision {
    /// Returns the next owner-assigned revision, or `None` on exhaustion.
    #[must_use]
    pub fn checked_next(self) -> Option<Self> {
        self.0
            .get()
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .map(Self)
    }

    /// Returns the first revision assigned by a grid owner.
    #[must_use]
    pub const fn first() -> Self {
        Self(NonZeroU64::MIN)
    }
}

/// Identity and immutable revision of one grid snapshot.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct BeatGridStamp {
    /// Returns the stable grid identity.
    #[field(get, copy)]
    grid_id: BeatGridId,
    /// Returns the immutable grid revision.
    #[field(get, copy)]
    revision: BeatGridRevision,
}

impl BeatGridStamp {
    /// Creates a composite grid stamp.
    #[must_use]
    pub const fn new(grid_id: BeatGridId, revision: BeatGridRevision) -> Self {
        Self { grid_id, revision }
    }
}
