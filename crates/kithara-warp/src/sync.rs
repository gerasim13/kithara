use std::num::NonZeroU64;

use super::{
    Beat, BeatMap, BeatMapId, BeatMapSnapshotError, MapPoint, MapRegion, MapStamp, SessionFrame,
};

mod applied;
mod member;
mod plan;
mod rejected;
mod topology;
pub use applied::SyncApplied;
pub use member::SyncMember;
pub use plan::{
    AlignmentCursor, AlignmentPlan, AlignmentPlanError, AlignmentRequest, AlignmentSource,
    AlignmentTransition, PlanSpan, PlanSpanSlot, PlanTransition, PlannedRenderSpan,
    SourceFrameRange, WarpPlan,
};
pub use rejected::SyncRejected;
pub use topology::{SyncGroupSnapshot, SyncGroupTopologyError, SyncMemberSnapshot};

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
    group_id: BeatMapId,
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

/// A beat on a source map aligned with a beat on a target map.
#[derive(Clone, Copy, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct BeatAlignment {
    /// Returns the point on the map being aligned.
    #[field(get, copy)]
    source: MapPoint<Beat>,
    /// Returns the corresponding point on the target map.
    #[field(get, copy)]
    target: MapPoint<Beat>,
}

impl BeatAlignment {
    /// Creates one directionally explicit alignment edge.
    #[must_use]
    pub const fn new(source: MapPoint<Beat>, target: MapPoint<Beat>) -> Self {
        Self { source, target }
    }
}

/// One operation routed through the live synchronization-group owner.
#[derive(Debug)]
pub enum SyncOperation<G: SyncGroup> {
    /// Applies one ordered ownership-tree transaction against an exact base.
    Topology {
        /// Topology identity and revision the caller observed.
        base: TopologyStamp,
        /// Ordered operations committed together or not at all.
        operations: Box<[TopologyOperation<G>]>,
    },
    /// Routes one playback transport operation through the resident Deck envelope.
    Transport {
        /// Stable Deck or Track map receiving the operation.
        target: BeatMapId,
        /// Exact Track load receiving the operation.
        load: LoadGeneration,
        /// Exact committed session transport state.
        transport: TransportRevision,
        /// Playback operation being routed.
        operation: TransportOperation,
    },
    /// Changes the synchronization intent of one Deck.
    Sync {
        /// Stable Deck map receiving the intent.
        target: BeatMapId,
        /// Exact Track load receiving the intent.
        load: LoadGeneration,
        /// Exact committed session transport state.
        transport: TransportRevision,
        /// Whether the affected PCM is prepared or already audible.
        source: AlignmentSource,
        /// Exact output frame at which the intent may take effect.
        activation: SessionFrame,
        /// Requested synchronization state transition.
        intent: SyncIntent,
    },
    /// Re-evaluates an active plan after one material control-plane change.
    Reconcile {
        /// Stable Deck map whose active plan is being re-evaluated.
        target: BeatMapId,
        /// Exact Track load whose active plan is being re-evaluated.
        load: LoadGeneration,
        /// Exact committed session transport state.
        transport: TransportRevision,
        /// Change that requires reconciliation.
        cause: ReconcileCause,
        /// Last source/output boundary consumed by the callback.
        frontier: PresentationFrontier,
    },
}

impl<G: SyncGroup> SyncOperation<G> {
    /// Returns the unique group or map targeted by this operation.
    #[must_use]
    pub const fn target(&self) -> BeatMapId {
        match self {
            Self::Topology { base, .. } => base.group_id,
            Self::Transport { target, .. }
            | Self::Sync { target, .. }
            | Self::Reconcile { target, .. } => *target,
        }
    }
}

/// One atomic ownership-tree operation.
#[derive(Debug)]
pub enum TopologyOperation<G: SyncGroup> {
    /// Attaches one exclusively owned member to a direct parent group.
    Attach {
        /// Live member transferred to the receiving group.
        member: SyncMember<G>,
    },
    /// Detaches one direct member from a parent group.
    Detach {
        /// Identity of the direct member being detached.
        member: BeatMapId,
    },
    /// Replaces one direct member atomically.
    Replace {
        /// Identity of the direct member being replaced.
        member: BeatMapId,
        /// New live member transferred to the parent group.
        replacement: SyncMember<G>,
    },
}

/// Runtime member category used by group-specific topology policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SyncMemberKind {
    /// An ordinary beat map, such as a loaded Track.
    Map,
    /// A nested synchronization group, such as a Deck below a Host.
    Group,
}

/// One playback operation that must cross the resident Deck envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TransportOperation {
    /// Prepare an exact source position before any affected PCM is audible.
    PrepareStart {
        /// Exact decoded source-frame destination.
        source_frame: u64,
    },
    /// Begin or resume playback.
    Play,
    /// Hold playback at the current frontier.
    Pause,
    /// Relocate playback to an exact decoded source-frame destination.
    Seek {
        /// Exact decoded source-frame destination.
        source_frame: u64,
    },
    /// End playback and retire the current render state.
    Stop,
}

/// Requested synchronization transition for one stable Deck.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SyncIntent {
    /// Start following Host tempo and phase.
    Enable,
    /// Stop future Host correction and latch the current effective settings.
    Disable,
    /// Snap immediately to Host tempo and phase as an explicit user action.
    AlignNow,
}

/// Material change that requires an active alignment plan to be reconsidered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ReconcileCause {
    /// A previously unavailable map became usable.
    MapAvailable,
    /// A newer map revision materially changed the active relation.
    MapRefined,
    /// The authoritative Host transport changed.
    TransportChanged,
    /// The recursive ownership tree changed.
    TopologyChanged,
}

/// An exact source/output boundary reached by the renderer.
#[derive(Clone, Copy, Debug, Eq, PartialEq, bon::Builder, fieldwork::Fieldwork)]
#[builder(state_mod(vis = "pub"))]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct RenderFrontier {
    /// Exclusive decoded source-frame boundary.
    #[field(get, copy)]
    source: u64,
    /// Exclusive session output-frame boundary.
    #[field(get, copy)]
    output: SessionFrame,
}

/// An exact source/output boundary consumed by the audio callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq, bon::Builder, fieldwork::Fieldwork)]
#[builder(state_mod(vis = "pub"))]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct PresentationFrontier {
    /// Exclusive decoded source-frame boundary actually consumed.
    #[field(get, copy)]
    source: u64,
    /// Exclusive session output-frame boundary actually consumed.
    #[field(get, copy)]
    output: SessionFrame,
}

/// A synchronization capability that may be unavailable in one implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SyncCapability {
    /// Ownership-tree mutation.
    Topology,
    /// Play, pause, seek, start preparation, and stop routing.
    Transport,
    /// Map-to-map tempo and phase alignment.
    Alignment,
    /// Continuity-preserving replacement of an active alignment plan.
    Reconciliation,
}

/// Result of validating and admitting one operation on the control plane.
#[derive(Clone, Copy, Debug, PartialEq)]
#[must_use]
#[non_exhaustive]
pub enum SyncAdmission {
    /// A control-plane-only topology transaction committed atomically.
    TopologyChanged {
        /// Identity assigned to the committed transaction.
        operation: SyncOperationId,
        /// Exact topology published by the transaction.
        topology: TopologyStamp,
    },
    /// A validated SYNC-off transport command may enter the existing PCM path.
    Accepted {
        /// Identity of the admitted operation.
        operation: SyncOperationId,
        /// Topology against which the operation was admitted.
        topology: TopologyStamp,
        /// Exact Track load authorized for dispatch.
        load: LoadGeneration,
        /// Exact committed session transport state authorized for dispatch.
        transport: TransportRevision,
    },
    /// A stamped plan is prepared for one exact render boundary.
    Prepared {
        /// Identity of the admitted operation.
        operation: SyncOperationId,
        /// Topology against which the operation was admitted.
        topology: TopologyStamp,
        /// Immutable plan prepared by the group.
        plan: AlignmentPlanRevision,
        /// Exact output boundary at which the plan takes effect.
        activation: SessionFrame,
    },
    /// The requested operation already matches committed state.
    Unchanged {
        /// Identity of the admitted operation.
        operation: SyncOperationId,
        /// Topology against which the operation was admitted.
        topology: TopologyStamp,
    },
    /// A later map revision may make the operation admissible.
    Deferred {
        /// Identity of the deferred operation.
        operation: SyncOperationId,
        /// Topology against which the operation was evaluated.
        topology: TopologyStamp,
        /// Map coverage required before the operation can be prepared.
        required: MapRegion,
    },
    /// The current implementation cannot perform this operation.
    Unavailable {
        /// Identity of the rejected operation.
        operation: SyncOperationId,
        /// Topology against which the operation was evaluated.
        topology: TopologyStamp,
        /// Missing synchronization capability.
        capability: SyncCapability,
    },
}

/// Canonical synchronization state observed from one live group.
#[derive(Clone, Copy, Debug, PartialEq)]
#[must_use]
#[non_exhaustive]
pub enum SyncStatusSnapshot {
    /// Host correction is disabled inside the resident signal path.
    Off { topology: TopologyStamp },
    /// The requested alignment needs map coverage not yet published.
    WaitingForMap {
        operation: SyncOperationId,
        topology: TopologyStamp,
        required: MapRegion,
    },
    /// A plan is admitted but its activation has not been acknowledged.
    Prepared {
        operation: SyncOperationId,
        topology: TopologyStamp,
        plan: AlignmentPlanRevision,
        activation: SessionFrame,
    },
    /// The requested behavior is not implemented by the current group.
    Unavailable {
        operation: SyncOperationId,
        topology: TopologyStamp,
        capability: SyncCapability,
    },
    /// The renderer has applied a continuity-preserving correction.
    Converging {
        applied: SyncApplied,
        phase_error_frames: f64,
    },
    /// The renderer is holding the target tempo and phase.
    Locked {
        applied: SyncApplied,
        phase_error_frames: f64,
    },
}

/// A synchronization operation violates the live group contract.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum SyncError {
    /// This group does not implement the requested operation yet.
    #[error("synchronization capability {capability:?} is unavailable")]
    CapabilityUnavailable { capability: SyncCapability },
    /// A topology transaction was based on another published revision.
    #[error("topology base is {given:?}, expected {expected:?}")]
    StaleTopology {
        expected: TopologyStamp,
        given: TopologyStamp,
    },
    /// No live group with the requested identity exists in this tree.
    #[error("synchronization group {group_id} was not found")]
    GroupNotFound { group_id: BeatMapId },
    /// A live map snapshot or publication belongs to another stable owner.
    #[error("map identity is {given}, expected {expected}")]
    MapIdentityMismatch {
        expected: BeatMapId,
        given: BeatMapId,
    },
    /// A group-map publication did not advance the owner's current revision.
    #[error("group map publication {given:?} does not advance {current:?}")]
    StaleMapRevision { current: MapStamp, given: MapStamp },
    /// A map owner attempted an invalid immutable snapshot transition.
    #[error(transparent)]
    BeatMapSnapshot(#[from] BeatMapSnapshotError),
    /// No direct member with the requested identity exists in this group.
    #[error("member {member_id} was not found in group {group_id}")]
    MemberNotFound {
        group_id: BeatMapId,
        member_id: BeatMapId,
    },
    /// A group policy does not admit this category of direct member.
    #[error("member {member_id} in group {group_id} has kind {given:?}, expected {expected:?}")]
    InvalidMemberKind {
        group_id: BeatMapId,
        member_id: BeatMapId,
        expected: SyncMemberKind,
        given: SyncMemberKind,
    },
    /// A topology owner cannot mint another revision.
    #[error("topology revision space is exhausted for group {group_id}")]
    TopologyRevisionExhausted { group_id: BeatMapId },
    /// A group owner cannot mint another operation identity.
    #[error("synchronization operation identity space is exhausted for group {group_id}")]
    OperationIdExhausted { group_id: BeatMapId },
    /// No prepared renderer operation can accept an acknowledgement.
    #[error("synchronization group has no prepared operation")]
    NoPreparedOperation,
    /// The renderer repeated an acknowledgement that was already committed.
    #[error("synchronization operation {operation} was already acknowledged")]
    DuplicateAcknowledgement { operation: SyncOperationId },
    /// The renderer acknowledged another operation than the prepared one.
    #[error("renderer acknowledged operation {given}, expected {expected}")]
    StaleAcknowledgement {
        expected: SyncOperationId,
        given: SyncOperationId,
    },
    /// One or more renderer acknowledgement stamps do not match the prepared plan.
    #[error("renderer acknowledgement {given:?} does not match {expected:?}")]
    AppliedMismatch {
        expected: Box<SyncApplied>,
        given: Box<SyncApplied>,
    },
    /// The candidate ownership tree violates a topology invariant.
    #[error(transparent)]
    Topology(#[from] SyncGroupTopologyError),
}

/// Live owner protocol for a recursive group of musical maps.
///
/// The topology's group-map stamp must equal `snapshot().stamp()`, and its
/// group identity must equal `id()`.
pub trait SyncGroup: BeatMap {
    /// Concrete synchronization-group type accepted as a direct child.
    type NestedGroup: SyncGroup;

    /// Returns one immutable topology snapshot for a complete calculation.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError`] when a live child violates the recursive topology
    /// contract while the observation is being materialized.
    fn topology(&self) -> Result<SyncGroupSnapshot, SyncError>;

    /// Validates and admits one operation without claiming audible application.
    ///
    /// # Errors
    ///
    /// Returns [`SyncRejected`] when the operation is invalid or unsupported;
    /// the rejected value retains ownership of the complete operation.
    fn transact(
        &mut self,
        operation: SyncOperation<Self::NestedGroup>,
    ) -> Result<SyncAdmission, SyncRejected<Self::NestedGroup>>;

    /// Returns the canonical control-plane view of this group's sync state.
    fn status(&self) -> SyncStatusSnapshot;

    /// Commits an operation as audibly applied and returns the resulting sync state.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError`] when the acknowledgement is stale, duplicate, or
    /// does not match the currently prepared operation.
    fn acknowledge(&mut self, applied: SyncApplied) -> Result<SyncStatusSnapshot, SyncError>;
}
