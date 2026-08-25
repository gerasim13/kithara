use std::num::NonZeroU64;

use crate::BeatMapId;

fn checked_next_revision(revision: NonZeroU64) -> Option<NonZeroU64> {
    revision.get().checked_add(1).and_then(NonZeroU64::new)
}

/// Monotonic revision of one synchronization-group topology.
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
pub struct TopologyRevision(NonZeroU64);

impl TopologyRevision {
    /// Returns the first revision assigned by a group owner.
    #[must_use]
    pub const fn first() -> Self {
        Self(NonZeroU64::MIN)
    }

    /// Returns the next owner-assigned revision, or `None` on exhaustion.
    #[must_use]
    pub fn checked_next(self) -> Option<Self> {
        checked_next_revision(self.0).map(Self)
    }
}

/// Monotonic identity of one synchronization operation.
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
pub struct SyncOperationId(NonZeroU64);

impl SyncOperationId {
    /// Returns the first operation identity assigned by a group owner.
    #[must_use]
    pub const fn first() -> Self {
        Self(NonZeroU64::MIN)
    }

    /// Returns the next owner-assigned identity, or `None` on exhaustion.
    #[must_use]
    pub fn checked_next(self) -> Option<Self> {
        checked_next_revision(self.0).map(Self)
    }
}

/// Monotonic revision of one immutable alignment plan.
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
pub struct AlignmentPlanRevision(NonZeroU64);

impl AlignmentPlanRevision {
    /// Returns the first revision assigned by a plan owner.
    #[must_use]
    pub const fn first() -> Self {
        Self(NonZeroU64::MIN)
    }

    /// Returns the next owner-assigned revision, or `None` on exhaustion.
    #[must_use]
    pub fn checked_next(self) -> Option<Self> {
        checked_next_revision(self.0).map(Self)
    }
}

/// Monotonic identity of one track load into a stable deck.
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
pub struct LoadGeneration(NonZeroU64);

impl LoadGeneration {
    /// Returns the first generation assigned by a deck owner.
    #[must_use]
    pub const fn first() -> Self {
        Self(NonZeroU64::MIN)
    }

    /// Returns the next owner-assigned generation, or `None` on exhaustion.
    #[must_use]
    pub fn checked_next(self) -> Option<Self> {
        checked_next_revision(self.0).map(Self)
    }
}

/// Monotonic revision of committed session transport state.
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
pub struct TransportRevision(NonZeroU64);

impl TransportRevision {
    /// Returns the first committed transport revision.
    #[must_use]
    pub const fn first() -> Self {
        Self(NonZeroU64::MIN)
    }

    /// Returns the next committed revision, or `None` on exhaustion.
    #[must_use]
    pub fn checked_next(self) -> Option<Self> {
        checked_next_revision(self.0).map(Self)
    }
}

/// Identity and immutable revision of one group topology snapshot.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct TopologyStamp {
    /// Returns the stable identity of the group map.
    #[field(get, copy)]
    pub(super) group_id: BeatMapId,
    /// Returns the immutable topology revision.
    #[field(get, copy)]
    revision: TopologyRevision,
}

impl TopologyStamp {
    /// Creates a composite topology stamp.
    #[must_use]
    pub const fn new(group_id: BeatMapId, revision: TopologyRevision) -> Self {
        Self { group_id, revision }
    }
}
