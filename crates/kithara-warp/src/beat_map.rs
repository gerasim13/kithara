mod query;

use std::num::NonZeroU64;

use kithara_platform::sync::Arc;
use portable_atomic::{AtomicU64, Ordering};

use super::{
    AlignmentPlan, AlignmentRequest, Beat, BeatEvidence, BeatsPerMinute, FrameUncertainty, MapAxis,
    MapPoint, MapPosition, MapRegion, Meter, MeterFacts, PlanTransition, PresentationFrontier,
    SegmentSet, SessionAnchor, SessionBeat, SessionFrame, SyncCapability, SyncError,
};

const SECONDS_PER_MINUTE: f64 = 60.0;

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
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub struct MapStamp {
    map_id: BeatMapId,
    revision: BeatMapRevision,
}

impl MapStamp {
    /// Creates a composite map stamp.
    #[must_use]
    pub const fn new(map_id: BeatMapId, revision: BeatMapRevision) -> Self {
        Self { map_id, revision }
    }

    /// Returns the stable map identity.
    #[must_use]
    pub const fn map_id(self) -> BeatMapId {
        self.map_id
    }

    /// Returns the immutable map revision.
    #[must_use]
    pub const fn revision(self) -> BeatMapRevision {
        self.revision
    }
}

/// Why a map cannot currently answer queries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum MapUnavailable {
    /// The supplied coordinate uses another map-native axis.
    AxisMismatch,
    /// No usable geometry is available for this map.
    NoGeometry,
    /// Beat geometry exists, but no meter evidence is available.
    NoMeter,
}

/// Lifecycle state of one immutable map snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum MapState {
    /// Partial geometry may gain coverage in later revisions.
    Building,
    /// The bounded map will not gain more coverage.
    Complete,
    /// The map describes a live coordinate domain.
    Live,
    /// The map cannot currently answer any query.
    Unavailable(MapUnavailable),
}

/// A resolved value and the evidence supporting it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct BeatEstimate<T> {
    value: T,
    evidence: BeatEvidence,
    uncertainty: FrameUncertainty,
    stamp: MapStamp,
}

impl<T> BeatEstimate<T> {
    pub(crate) const fn new(
        value: T,
        evidence: BeatEvidence,
        uncertainty: FrameUncertainty,
        stamp: MapStamp,
    ) -> Self {
        Self {
            value,
            evidence,
            uncertainty,
            stamp,
        }
    }

    /// Returns the resolved value.
    #[must_use]
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// Returns how the value was established.
    #[must_use]
    pub const fn evidence(&self) -> BeatEvidence {
        self.evidence
    }

    /// Returns maximum absolute error in map-native frames.
    #[must_use]
    pub const fn uncertainty(&self) -> FrameUncertainty {
        self.uncertainty
    }

    /// Returns the snapshot identity and revision used by the estimate.
    #[must_use]
    pub const fn stamp(&self) -> MapStamp {
        self.stamp
    }
}

/// Typed result of a musical-map query.
#[derive(Clone, Debug, PartialEq)]
#[must_use]
#[non_exhaustive]
pub enum MapQuery<T> {
    /// The snapshot resolved the requested coordinate.
    Resolved(T),
    /// A later revision may cover the requested map region.
    Uncovered { required: MapRegion },
    /// The complete bounded map proves the coordinate is outside its domain.
    OutsideDomain,
    /// The supplied point belongs to another map identity or revision.
    Stale { expected: MapStamp, given: MapStamp },
    /// The map cannot answer the query for the stated reason.
    Unavailable(MapUnavailable),
}

/// Read-only musical-coordinate protocol shared by asset and host maps.
pub trait BeatMap: Send + Sync + 'static {
    /// Returns the stable identity of this map owner.
    fn id(&self) -> BeatMapId;

    /// Returns one immutable snapshot for a complete multi-step calculation.
    ///
    /// Implementors must preserve `snapshot().id() == id()`. Revisions for one
    /// map identity never move backward, and every published replacement uses
    /// a later revision than the snapshot it replaces.
    fn snapshot(&self) -> BeatMapSnapshot;

    /// Compiles a stamped source-to-target alignment plan.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError`] when map coverage, stamps, coordinates, policy, or
    /// the implementation's alignment capability cannot satisfy `request`.
    fn align_to(
        &self,
        target: &dyn BeatMap,
        request: AlignmentRequest,
    ) -> Result<AlignmentPlan, SyncError>;

    /// Reconciles a newer map observation without changing already audible PCM.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError`] when the active plan or frontier is stale, or a
    /// continuity-preserving successor cannot be compiled.
    fn reconcile_to(
        &self,
        target: &dyn BeatMap,
        active: &AlignmentPlan,
        frontier: PresentationFrontier,
    ) -> Result<PlanTransition, SyncError>;
}

/// One immutable, revisioned musical-map observation.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct BeatMapSnapshot {
    pub(crate) data: Arc<BeatMapSnapshotData>,
}

impl BeatMap for BeatMapSnapshot {
    delegate::delegate! {
        to self {
            #[call(id)]
            fn id(&self) -> BeatMapId;
            #[call(clone)]
            fn snapshot(&self) -> BeatMapSnapshot;
        }
    }

    fn align_to(
        &self,
        _target: &dyn BeatMap,
        _request: AlignmentRequest,
    ) -> Result<AlignmentPlan, SyncError> {
        Err(SyncError::CapabilityUnavailable {
            capability: SyncCapability::Alignment,
        })
    }

    fn reconcile_to(
        &self,
        _target: &dyn BeatMap,
        _active: &AlignmentPlan,
        _frontier: PresentationFrontier,
    ) -> Result<PlanTransition, SyncError> {
        Err(SyncError::CapabilityUnavailable {
            capability: SyncCapability::Reconciliation,
        })
    }
}

/// A caller-supplied snapshot violates the public musical-map contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum BeatMapSnapshotError {
    /// The proposed replacement was derived from any snapshot but the current one.
    #[error("beat map successor is stale: expected {expected:?}, got {given:?}")]
    Stale { expected: MapStamp, given: MapStamp },
    /// A successor changed its native coordinate axis outside a host restart.
    #[error("beat map successor changed axis from {expected:?} to {given:?}")]
    AxisChanged { expected: MapAxis, given: MapAxis },
    /// The lifecycle state is incompatible with the snapshot coordinate axis.
    #[error("state {state:?} is invalid for segment geometry on axis {axis:?}")]
    InvalidState { axis: MapAxis, state: MapState },
    /// A complete bounded map cannot return to an incomplete lifecycle state.
    #[error("beat map cannot transition from {from:?} to {to:?}")]
    InvalidTransition { from: MapState, to: MapState },
    /// The map exhausted its monotonic revision space.
    #[error("beat map revision space is exhausted")]
    RevisionExhausted,
}

#[derive(Debug)]
pub(crate) struct BeatMapSnapshotData {
    pub(crate) id: BeatMapId,
    pub(crate) revision: BeatMapRevision,
    pub(crate) state: MapState,
    pub(crate) axis: MapAxis,
    pub(crate) geometry: BeatMapGeometry,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BeatMapGeometry {
    Segments(SegmentSet),
    Host {
        anchor: SessionAnchor,
        meter: Option<MeterFacts>,
    },
}

impl BeatMapSnapshot {
    /// Creates the first validated segment-backed snapshot for a map owner.
    ///
    /// # Errors
    ///
    /// Returns [`BeatMapSnapshotError`] when `state` is invalid for the
    /// coordinate axis carried by `segments`.
    pub fn initial(
        id: BeatMapId,
        state: MapState,
        segments: SegmentSet,
    ) -> Result<Self, BeatMapSnapshotError> {
        Self::try_new_segments(id, BeatMapRevision::first(), state, segments)
    }

    fn try_new_segments(
        id: BeatMapId,
        revision: BeatMapRevision,
        state: MapState,
        segments: SegmentSet,
    ) -> Result<Self, BeatMapSnapshotError> {
        let axis = segments.axis();
        if matches!(
            (axis, state),
            (MapAxis::Asset(_), MapState::Live)
                | (MapAxis::Host(_), MapState::Complete)
                | (_, MapState::Unavailable(_))
        ) {
            return Err(BeatMapSnapshotError::InvalidState { axis, state });
        }
        Ok(Self::new_segments(id, revision, state, segments))
    }

    fn new_segments(
        id: BeatMapId,
        revision: BeatMapRevision,
        state: MapState,
        segments: SegmentSet,
    ) -> Self {
        let axis = segments.axis();
        Self {
            data: Arc::new(BeatMapSnapshotData {
                id,
                revision,
                state,
                axis,
                geometry: BeatMapGeometry::Segments(segments),
            }),
        }
    }

    pub(crate) fn new_host(
        id: BeatMapId,
        revision: BeatMapRevision,
        axis: MapAxis,
        anchor: SessionAnchor,
        meter: Option<MeterFacts>,
    ) -> Self {
        Self {
            data: Arc::new(BeatMapSnapshotData {
                id,
                revision,
                state: MapState::Live,
                axis,
                geometry: BeatMapGeometry::Host { anchor, meter },
            }),
        }
    }
}

impl PartialEq for BeatMapSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
            && self.revision() == other.revision()
            && self.state() == other.state()
            && self.axis() == other.axis()
            && self.data.geometry == other.data.geometry
    }
}
