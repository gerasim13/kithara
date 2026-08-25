use super::{
    AlignmentPlanRevision, SyncAdmission, SyncApplied, SyncCapability, SyncGroupSnapshot,
    SyncGroupTopologyError, SyncMemberKind, SyncOperation, SyncOperationId, SyncRejected,
    TopologyStamp,
};
use crate::{BeatMap, BeatMapId, BeatMapSnapshotError, MapRegion, MapStamp, SessionFrame};

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
