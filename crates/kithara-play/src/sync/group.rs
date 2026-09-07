use std::num::NonZeroU32;

use kithara_warp::{
    AssetFrame, BeatGrid, BeatGridId, BeatGridQuery, BeatGridRevision, BeatGridSnapshot,
    BeatGridStamp, BeatGridState, BeatsPerMinute, MapAxis, MapPoint, MapPosition, MapRegion,
    SessionAxis, SessionEpoch, SessionFrame, SyncAdmission, SyncApplied, SyncCapability, SyncError,
    SyncGroup, SyncGroupSnapshot, SyncMember, SyncMemberKind, SyncMode, SyncOperation,
    SyncOperationId, SyncRejected, SyncStatusSnapshot, TopologyRevision, TopologyStamp,
};

use super::{TempoSource, topology::materialize_topology, transaction};

/// Canonical mutable state for one recursive synchronization group.
///
/// `G` is the concrete nested-group representation. The group owns every live
/// member exclusively; callers interact through transactions or closure-based
/// access so member references cannot escape the owning lock.
pub struct GroupState<G: SyncGroup<NestedGroup = G>> {
    grid: BeatGridSnapshot,
    next_operation: Option<SyncOperationId>,
    unavailable: Option<(SyncOperationId, SyncCapability)>,
    waiting: Option<(SyncOperationId, MapRegion)>,
    member_kind: SyncMemberKind,
    mode: SyncMode,
    tempo: TempoSource,
    topology_revision: TopologyRevision,
    members: Vec<SyncMember<G>>,
}

impl<G: SyncGroup<NestedGroup = G>> GroupState<G> {
    /// Creates an empty group around an already-published grid.
    #[must_use]
    pub fn new(grid: BeatGridSnapshot, member_kind: SyncMemberKind, mode: SyncMode) -> Self {
        Self {
            grid,
            mode,
            member_kind,
            members: Vec::new(),
            next_operation: Some(SyncOperationId::first()),
            tempo: TempoSource::Inherited,
            topology_revision: TopologyRevision::first(),
            unavailable: None,
            waiting: None,
        }
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
                given,
                current: self.grid.stamp(),
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

    /// The tempo a `Disable` latches from the group or its first live grid.
    pub(crate) fn seed_local_tempo(&self) -> Option<BeatsPerMinute> {
        if let TempoSource::Local(tempo) = self.tempo {
            return Some(tempo);
        }
        let live = (self.grid.state() == BeatGridState::Live).then(|| {
            let origin = MapPoint::new(
                self.grid.stamp(),
                MapPosition::Session(SessionFrame::new(0)),
            );
            self.grid.tempo_at(origin)
        });
        let member = self.members.iter().find_map(|member| match member {
            SyncMember::Grid { grid, .. } => {
                let snapshot = grid.snapshot();
                let origin = MapPoint::new(
                    snapshot.stamp(),
                    MapPosition::Asset(AssetFrame::new(0.0).ok()?),
                );
                Some(snapshot.tempo_at(origin))
            }
            SyncMember::Group { .. } => None,
        });
        live.or(member).and_then(|query| match query {
            BeatGridQuery::Resolved(estimate) => Some(*estimate.value()),
            _ => None,
        })
    }

    /// Creates an empty group whose session-axis grid is not available yet.
    #[must_use]
    pub fn unavailable(
        id: BeatGridId,
        sample_rate: NonZeroU32,
        epoch: SessionEpoch,
        member_kind: SyncMemberKind,
        mode: SyncMode,
    ) -> Self {
        Self::new(
            BeatGridSnapshot::unavailable(
                id,
                BeatGridRevision::first(),
                MapAxis::Session(SessionAxis::new(sample_rate, epoch)),
            ),
            member_kind,
            mode,
        )
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

    fn acknowledge(&mut self, _applied: SyncApplied) -> Result<SyncStatusSnapshot, SyncError> {
        Err(SyncError::NoPreparedOperation)
    }

    fn status(&self) -> SyncStatusSnapshot {
        transaction::status(
            TopologyStamp::new(self.grid.id(), self.topology_revision),
            self.unavailable,
            self.waiting,
        )
    }

    fn topology(&self) -> Result<SyncGroupSnapshot, SyncError> {
        materialize_topology(&self.grid, self.topology_revision, &self.members)
    }

    fn transact(&mut self, operation: SyncOperation<G>) -> Result<SyncAdmission, SyncRejected<G>> {
        let seed = self.seed_local_tempo();
        transaction::transact(
            &self.grid,
            transaction::GroupSlots {
                next_operation: &mut self.next_operation,
                unavailable: &mut self.unavailable,
                waiting: &mut self.waiting,
                mode: &mut self.mode,
                tempo: &mut self.tempo,
                topology_revision: &mut self.topology_revision,
                members: &mut self.members,
            },
            self.member_kind,
            seed,
            operation,
        )
    }
}

fn is_successor_epoch(current: SessionEpoch, next: SessionEpoch) -> bool {
    u64::from(current)
        .checked_add(1)
        .is_some_and(|successor| successor == u64::from(next))
}
