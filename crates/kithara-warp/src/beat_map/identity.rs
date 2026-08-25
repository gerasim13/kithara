use std::num::NonZeroU64;

use portable_atomic::{AtomicU64, Ordering};

/// Stable identity of one musical map owner.
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
pub struct BeatMapId(NonZeroU64);

impl BeatMapId {
    /// Allocates an identity unique to this process.
    ///
    /// Every map owner uses this allocation site so points from independent
    /// sessions and registries cannot acquire equal stamps accidentally.
    ///
    /// # Errors
    ///
    /// Returns [`BeatMapIdAllocationError`] after the non-zero identity space
    /// has been exhausted.
    pub fn allocate() -> Result<Self, BeatMapIdAllocationError> {
        static NEXT: AtomicU64 = AtomicU64::new(1);

        let value = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                (current != 0).then(|| current.wrapping_add(1))
            })
            .map_err(|_| BeatMapIdAllocationError)?;
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(BeatMapIdAllocationError)
    }
}

/// The process-wide musical-map identity space is exhausted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("beat map identity space is exhausted")]
pub struct BeatMapIdAllocationError;

/// Monotonic revision of one [`BeatMapId`].
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
pub struct BeatMapRevision(NonZeroU64);

impl BeatMapRevision {
    /// Returns the first revision assigned by a map owner.
    #[must_use]
    pub const fn first() -> Self {
        Self(NonZeroU64::MIN)
    }

    /// Returns the next owner-assigned revision, or `None` on exhaustion.
    #[must_use]
    pub fn checked_next(self) -> Option<Self> {
        self.0
            .get()
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .map(Self)
    }
}

/// Identity and immutable revision of one map snapshot.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct MapStamp {
    /// Returns the stable map identity.
    #[field(get, copy)]
    map_id: BeatMapId,
    /// Returns the immutable map revision.
    #[field(get, copy)]
    revision: BeatMapRevision,
}

impl MapStamp {
    /// Creates a composite map stamp.
    #[must_use]
    pub const fn new(map_id: BeatMapId, revision: BeatMapRevision) -> Self {
        Self { map_id, revision }
    }
}
