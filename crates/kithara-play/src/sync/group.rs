use std::num::NonZeroU32;

use kithara_warp::{
    BeatGrid, BeatGridId, BeatGridRevision, BeatGridSnapshot, BeatGridStamp, BeatGridState,
    MapAxis, SessionAxis, SessionEpoch, SyncAdmission, SyncApplied, SyncCapability, SyncError,
    SyncGroup, SyncGroupSnapshot, SyncMember, SyncMemberKind, SyncOperation, SyncOperationId,
    SyncRejected, SyncStatusSnapshot, TopologyRevision, TopologyStamp,
};

use super::{topology::materialize_topology, transaction};

/// Canonical mutable state for one recursive synchronization group.
///
/// `G` is the concrete nested-group representation. The group owns every live
/// member exclusively; callers interact through transactions or closure-based
/// access so member references cannot escape the owning lock.
pub struct GroupState<G: SyncGroup<NestedGroup = G>> {
    grid: BeatGridSnapshot,
    members: Vec<SyncMember<G>>,
    next_operation: Option<SyncOperationId>,
    topology_revision: TopologyRevision,
    unavailable: Option<(SyncOperationId, SyncCapability)>,
    member_kind: SyncMemberKind,
}

impl<G: SyncGroup<NestedGroup = G>> GroupState<G> {
    /// Creates an empty group around an already-published grid.
    #[must_use]
    pub fn new(grid: BeatGridSnapshot, member_kind: SyncMemberKind) -> Self {
        Self {
            grid,
            members: Vec::new(),
            next_operation: Some(SyncOperationId::first()),
            topology_revision: TopologyRevision::first(),
            unavailable: None,
            member_kind,
        }
    }

    /// Creates an empty group whose session-axis grid is not available yet.
    #[must_use]
    pub fn unavailable(
        id: BeatGridId,
        sample_rate: NonZeroU32,
        epoch: SessionEpoch,
        member_kind: SyncMemberKind,
    ) -> Self {
        Self::new(
            BeatGridSnapshot::unavailable(
                id,
                BeatGridRevision::first(),
                MapAxis::Session(SessionAxis::new(sample_rate, epoch)),
            ),
            member_kind,
        )
    }

    /// Publishes a later immutable grid snapshot for this stable owner.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError`] when the candidate changes identity or axis, moves
    /// the revision backwards, or violates the group-grid lifecycle.
    pub fn publish_grid(&mut self, candidate: BeatGridSnapshot) -> Result<(), SyncError> {
        let given = candidate.stamp();
        if given.grid_id() != self.grid.id() {
            return Err(SyncError::GridIdentityMismatch {
                expected: self.grid.id(),
                given: given.grid_id(),
            });
        }
        let candidate_state = candidate.state();
        let candidate_axis = candidate.axis();
        let current_state = self.grid.state();
        let expected_axis = self.grid.axis();
        if given == self.grid.stamp() {
            return Ok(());
        }
        if given.revision() <= self.grid.revision() {
            return Err(SyncError::StaleGridRevision {
                current: self.grid.stamp(),
                given,
            });
        }
        if !matches!(
            candidate_state,
            BeatGridState::Live | BeatGridState::Unavailable(_)
        ) {
            return Err(SyncError::InvalidGroupGridState {
                state: candidate_state,
            });
        }
        let axis_is_valid = match (expected_axis, candidate_axis) {
            (MapAxis::Session(current), MapAxis::Session(next))
                if is_successor_epoch(current.epoch(), next.epoch())
                    && matches!(candidate_state, BeatGridState::Unavailable(_)) =>
            {
                true
            }
            (MapAxis::Session(current), MapAxis::Session(next))
                if next.epoch() == current.epoch() =>
            {
                match (current_state, candidate_state) {
                    (BeatGridState::Live, BeatGridState::Live)
                    | (BeatGridState::Unavailable(_), BeatGridState::Unavailable(_)) => {
                        current.sample_rate() == next.sample_rate()
                    }
                    (BeatGridState::Unavailable(_), BeatGridState::Live) => true,
                    (BeatGridState::Live, BeatGridState::Unavailable(_)) => {
                        return Err(SyncError::InvalidGroupGridTransition {
                            from: current_state,
                            to: candidate_state,
                        });
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        if !axis_is_valid {
            return Err(SyncError::GridAxisChanged {
                expected: expected_axis,
                given: candidate_axis,
            });
        }
        self.grid = candidate;
        Ok(())
    }

    /// Publishes a later unavailable session-axis snapshot.
    ///
    /// # Errors
    ///
    /// Forwards validation failures from [`Self::publish_grid`].
    pub fn publish_unavailable_grid(
        &mut self,
        stamp: BeatGridStamp,
        sample_rate: NonZeroU32,
        epoch: SessionEpoch,
    ) -> Result<(), SyncError> {
        self.publish_grid(BeatGridSnapshot::unavailable(
            stamp.grid_id(),
            stamp.revision(),
            MapAxis::Session(SessionAxis::new(sample_rate, epoch)),
        ))
    }

    /// Executes `dispatch` against one direct nested group without exposing a
    /// reference outside the call.
    pub fn with_group<R, F>(&self, id: BeatGridId, dispatch: F) -> Option<R>
    where
        R: 'static,
        F: FnOnce(&G) -> R,
    {
        let group = self.members.iter().find_map(|member| match member {
            SyncMember::Group { group, .. } if group.id() == id => Some(group.as_ref()),
            SyncMember::Grid { .. } | SyncMember::Group { .. } => None,
        })?;
        Some(dispatch(group))
    }
}

impl<G: SyncGroup<NestedGroup = G>> BeatGrid for GroupState<G> {
    delegate::delegate! {
        to self.grid {
            fn id(&self) -> BeatGridId;
            #[call(clone)]
            fn snapshot(&self) -> BeatGridSnapshot;
        }
    }
}

impl<G: SyncGroup<NestedGroup = G>> SyncGroup for GroupState<G> {
    type NestedGroup = G;

    fn topology(&self) -> Result<SyncGroupSnapshot, SyncError> {
        materialize_topology(&self.grid, self.topology_revision, &self.members)
    }

    fn transact(&mut self, operation: SyncOperation<G>) -> Result<SyncAdmission, SyncRejected<G>> {
        transaction::transact(
            &self.grid,
            &mut self.topology_revision,
            &mut self.members,
            &mut self.next_operation,
            &mut self.unavailable,
            self.member_kind,
            operation,
        )
    }

    fn status(&self) -> SyncStatusSnapshot {
        transaction::status(
            TopologyStamp::new(self.grid.id(), self.topology_revision),
            self.unavailable,
        )
    }

    fn acknowledge(&mut self, _applied: SyncApplied) -> Result<SyncStatusSnapshot, SyncError> {
        Err(SyncError::NoPreparedOperation)
    }
}

fn is_successor_epoch(current: SessionEpoch, next: SessionEpoch) -> bool {
    u64::from(current)
        .checked_add(1)
        .is_some_and(|successor| successor == u64::from(next))
}
