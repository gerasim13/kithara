use std::num::NonZeroU32;

use kithara_warp::{
    BeatGrid, BeatGridId, BeatGridRevision, BeatGridSnapshot, BeatGridStamp, BeatGridState,
    MapAxis, SessionAxis, SessionEpoch, SyncAdmission, SyncApplied, SyncCapability, SyncError,
    SyncGroup, SyncGroupSnapshot, SyncGroupTopologyError, SyncIntent, SyncMember, SyncMemberKind,
    SyncMemberSnapshot, SyncOperation, SyncOperationId, SyncRejected, SyncStatusSnapshot,
    TopologyOperation, TopologyRevision, TopologyStamp,
};

use super::state::Deck;

pub(super) struct Host {
    grid: BeatGridSnapshot,
    decks: Vec<SyncMember<Deck>>,
    next_operation: Option<SyncOperationId>,
    topology_revision: TopologyRevision,
    unavailable: Option<(SyncOperationId, SyncCapability)>,
}

#[derive(Clone, Copy)]
enum GroupKind {
    Host,
    Deck,
}

impl Host {
    pub(super) fn new(id: BeatGridId, sample_rate: NonZeroU32) -> Self {
        Self {
            grid: unavailable_grid(id, sample_rate, SessionEpoch::new(0)),
            decks: Vec::new(),
            next_operation: Some(SyncOperationId::first()),
            topology_revision: TopologyRevision::first(),
            unavailable: None,
        }
    }

    pub(super) fn publish_grid(&mut self, candidate: BeatGridSnapshot) -> Result<(), SyncError> {
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

    pub(super) fn publish_unavailable_grid(
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

    pub(super) fn deck_count(&self) -> usize {
        self.decks.len()
    }

    pub(super) fn deck(&self, index: usize) -> Option<&Deck> {
        match self.decks.get(index)? {
            SyncMember::Group { group, .. } => Some(group),
            SyncMember::Grid { .. } => None,
        }
    }

    pub(super) fn deck_mut(&mut self, index: usize) -> Option<&mut Deck> {
        match self.decks.get_mut(index)? {
            SyncMember::Group { group, .. } => Some(group),
            SyncMember::Grid { .. } => None,
        }
    }

    pub(super) fn deck_index(&self, player_id: u64) -> Option<usize> {
        self.decks.iter().position(|member| {
            matches!(member, SyncMember::Group { group, .. } if group.player_id == player_id)
        })
    }

    pub(super) fn decks(&self) -> impl Iterator<Item = &Deck> {
        self.decks.iter().filter_map(|member| match member {
            SyncMember::Group { group, .. } => Some(group.as_ref()),
            SyncMember::Grid { .. } => None,
        })
    }
}

fn is_successor_epoch(current: SessionEpoch, next: SessionEpoch) -> bool {
    u64::from(current)
        .checked_add(1)
        .is_some_and(|successor| successor == u64::from(next))
}

pub(super) fn unavailable_grid(
    id: BeatGridId,
    sample_rate: NonZeroU32,
    epoch: SessionEpoch,
) -> BeatGridSnapshot {
    BeatGridSnapshot::unavailable(
        id,
        BeatGridRevision::first(),
        MapAxis::Session(SessionAxis::new(sample_rate, epoch)),
    )
}

impl BeatGrid for Host {
    delegate::delegate! {
        to self.grid {
            fn id(&self) -> BeatGridId;
            #[call(clone)]
            fn snapshot(&self) -> BeatGridSnapshot;
        }
    }
}

impl BeatGrid for Deck {
    delegate::delegate! {
        to self.grid {
            fn id(&self) -> BeatGridId;
            #[call(clone)]
            fn snapshot(&self) -> BeatGridSnapshot;
        }
    }
}

impl SyncGroup for Host {
    type NestedGroup = Deck;

    fn topology(&self) -> Result<SyncGroupSnapshot, SyncError> {
        materialize_topology(&self.grid, self.topology_revision, &self.decks)
    }

    fn transact(
        &mut self,
        operation: SyncOperation<Deck>,
    ) -> Result<SyncAdmission, SyncRejected<Deck>> {
        transact(
            &self.grid,
            &mut self.topology_revision,
            &mut self.decks,
            &mut self.next_operation,
            &mut self.unavailable,
            GroupKind::Host,
            operation,
        )
    }

    fn status(&self) -> SyncStatusSnapshot {
        status(
            TopologyStamp::new(self.grid.id(), self.topology_revision),
            self.unavailable,
        )
    }

    fn acknowledge(&mut self, _applied: SyncApplied) -> Result<SyncStatusSnapshot, SyncError> {
        Err(SyncError::NoPreparedOperation)
    }
}

impl SyncGroup for Deck {
    type NestedGroup = Self;

    fn topology(&self) -> Result<SyncGroupSnapshot, SyncError> {
        materialize_topology(&self.grid, self.topology_revision, &self.tracks)
    }

    fn transact(
        &mut self,
        operation: SyncOperation<Self>,
    ) -> Result<SyncAdmission, SyncRejected<Self>> {
        transact(
            &self.grid,
            &mut self.topology_revision,
            &mut self.tracks,
            &mut self.next_operation,
            &mut self.unavailable,
            GroupKind::Deck,
            operation,
        )
    }

    fn status(&self) -> SyncStatusSnapshot {
        status(
            TopologyStamp::new(self.grid.id(), self.topology_revision),
            self.unavailable,
        )
    }

    fn acknowledge(&mut self, _applied: SyncApplied) -> Result<SyncStatusSnapshot, SyncError> {
        Err(SyncError::NoPreparedOperation)
    }
}

fn materialize_topology(
    grid: &BeatGridSnapshot,
    revision: TopologyRevision,
    members: &[SyncMember<Deck>],
) -> Result<SyncGroupSnapshot, SyncError> {
    let members = members
        .iter()
        .map(|member| member.snapshot_for(grid))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SyncGroupSnapshot::try_new(grid.clone(), revision, members)?)
}

fn status(
    topology: TopologyStamp,
    unavailable: Option<(SyncOperationId, SyncCapability)>,
) -> SyncStatusSnapshot {
    unavailable.map_or(
        SyncStatusSnapshot::Off { topology },
        |(operation, capability)| SyncStatusSnapshot::Unavailable {
            operation,
            topology,
            capability,
        },
    )
}

fn transact(
    grid: &BeatGridSnapshot,
    topology_revision: &mut TopologyRevision,
    members: &mut Vec<SyncMember<Deck>>,
    next_operation: &mut Option<SyncOperationId>,
    unavailable: &mut Option<(SyncOperationId, SyncCapability)>,
    kind: GroupKind,
    operation: SyncOperation<Deck>,
) -> Result<SyncAdmission, SyncRejected<Deck>> {
    let target = operation.target();
    let topology_operation = matches!(&operation, SyncOperation::Topology { .. });
    if target == grid.id() || (!topology_operation && owns_direct_grid(members, target)) {
        return transact_local(
            grid,
            topology_revision,
            members,
            next_operation,
            unavailable,
            kind,
            operation,
        );
    }

    let topology_change = matches!(
        &operation,
        SyncOperation::Topology { operations, .. } if !operations.is_empty()
    );
    if let SyncOperation::Topology { base, operations } = &operation {
        let root = match materialize_topology(grid, *topology_revision, members) {
            Ok(root) => root,
            Err(error) => return Err(SyncRejected::new(error, operation)),
        };
        if let Err(error) = preview_topology(&root, *base, operations, kind) {
            return Err(SyncRejected::new(error, operation));
        }
    }
    let parent_revision = match topology_change
        .then(|| next_topology_revision(grid.id(), *topology_revision))
        .transpose()
    {
        Ok(revision) => revision,
        Err(error) => return Err(SyncRejected::new(error, operation)),
    };
    let group = match routed_group(members, target) {
        Ok(Some(group)) => group,
        Ok(None) => {
            return Err(SyncRejected::new(
                SyncError::GroupNotFound { group_id: target },
                operation,
            ));
        }
        Err(error) => return Err(SyncRejected::new(error, operation)),
    };
    let admission = group.transact(operation)?;
    if matches!(admission, SyncAdmission::TopologyChanged { .. })
        && let Some(revision) = parent_revision
    {
        *topology_revision = revision;
    }
    Ok(admission)
}

fn transact_local(
    grid: &BeatGridSnapshot,
    topology_revision: &mut TopologyRevision,
    members: &mut Vec<SyncMember<Deck>>,
    next_operation: &mut Option<SyncOperationId>,
    unavailable: &mut Option<(SyncOperationId, SyncCapability)>,
    kind: GroupKind,
    operation: SyncOperation<Deck>,
) -> Result<SyncAdmission, SyncRejected<Deck>> {
    match &operation {
        SyncOperation::Topology { .. } => transact_topology(
            grid,
            topology_revision,
            members,
            next_operation,
            kind,
            operation,
        ),
        SyncOperation::Sync {
            intent: SyncIntent::Disable,
            ..
        } => {
            let operation_id = match take_operation(grid.id(), next_operation) {
                Ok(operation_id) => operation_id,
                Err(error) => return Err(SyncRejected::new(error, operation)),
            };
            *unavailable = None;
            Ok(SyncAdmission::Unchanged {
                operation: operation_id,
                topology: TopologyStamp::new(grid.id(), *topology_revision),
            })
        }
        SyncOperation::Transport {
            load, transport, ..
        } => {
            let load = *load;
            let transport = *transport;
            let operation_id = match take_operation(grid.id(), next_operation) {
                Ok(operation_id) => operation_id,
                Err(error) => return Err(SyncRejected::new(error, operation)),
            };
            *unavailable = None;
            Ok(SyncAdmission::Accepted {
                operation: operation_id,
                topology: TopologyStamp::new(grid.id(), *topology_revision),
                load,
                transport,
            })
        }
        SyncOperation::Sync { .. } => preserve_rejected(
            unavailable_admission(
                grid.id(),
                *topology_revision,
                next_operation,
                unavailable,
                SyncCapability::Alignment,
            ),
            operation,
        ),
        SyncOperation::Reconcile { .. } => preserve_rejected(
            unavailable_admission(
                grid.id(),
                *topology_revision,
                next_operation,
                unavailable,
                SyncCapability::Reconciliation,
            ),
            operation,
        ),
    }
}

fn preserve_rejected(
    result: Result<SyncAdmission, SyncError>,
    operation: SyncOperation<Deck>,
) -> Result<SyncAdmission, SyncRejected<Deck>> {
    result.map_err(|error| SyncRejected::new(error, operation))
}

fn unavailable_admission(
    group_id: BeatGridId,
    topology_revision: TopologyRevision,
    next_operation: &mut Option<SyncOperationId>,
    unavailable: &mut Option<(SyncOperationId, SyncCapability)>,
    capability: SyncCapability,
) -> Result<SyncAdmission, SyncError> {
    let operation = take_operation(group_id, next_operation)?;
    let topology = TopologyStamp::new(group_id, topology_revision);
    *unavailable = Some((operation, capability));
    Ok(SyncAdmission::Unavailable {
        operation,
        topology,
        capability,
    })
}

fn transact_topology(
    grid: &BeatGridSnapshot,
    topology_revision: &mut TopologyRevision,
    members: &mut Vec<SyncMember<Deck>>,
    next_operation: &mut Option<SyncOperationId>,
    kind: GroupKind,
    operation: SyncOperation<Deck>,
) -> Result<SyncAdmission, SyncRejected<Deck>> {
    let (base, operations) = match operation {
        SyncOperation::Topology { base, operations } => (base, operations),
        operation => {
            return Err(SyncRejected::new(
                SyncError::CapabilityUnavailable {
                    capability: SyncCapability::Topology,
                },
                operation,
            ));
        }
    };
    let reject =
        |error, operations| SyncRejected::new(error, SyncOperation::Topology { base, operations });
    let expected = TopologyStamp::new(grid.id(), *topology_revision);
    if base != expected {
        return Err(reject(
            SyncError::StaleTopology {
                expected,
                given: base,
            },
            operations,
        ));
    }

    let operation_id = match (*next_operation).ok_or_else(|| SyncError::OperationIdExhausted {
        group_id: grid.id(),
    }) {
        Ok(operation_id) => operation_id,
        Err(error) => return Err(reject(error, operations)),
    };
    if operations.is_empty() {
        advance_operation(next_operation);
        return Ok(SyncAdmission::Unchanged {
            operation: operation_id,
            topology: expected,
        });
    }

    let revision = match next_topology_revision(grid.id(), *topology_revision) {
        Ok(revision) => revision,
        Err(error) => return Err(reject(error, operations)),
    };
    if let Err(error) = validate_topology_candidate(grid, revision, members, &operations, kind) {
        return Err(reject(error, operations));
    }
    apply_topology_operations(members, operations);
    *topology_revision = revision;
    advance_operation(next_operation);
    Ok(SyncAdmission::TopologyChanged {
        operation: operation_id,
        topology: TopologyStamp::new(grid.id(), revision),
    })
}

fn validate_topology_candidate(
    grid: &BeatGridSnapshot,
    revision: TopologyRevision,
    members: &[SyncMember<Deck>],
    operations: &[TopologyOperation<Deck>],
    kind: GroupKind,
) -> Result<(), SyncError> {
    let mut candidate: Vec<SyncMemberSnapshot> = members
        .iter()
        .map(|member| member.snapshot_for(grid))
        .collect::<Result<_, _>>()?;

    for operation in operations {
        match operation {
            TopologyOperation::Attach { member } => {
                let snapshot = validate_incoming_member(grid, member, kind)?;
                candidate.push(snapshot);
            }
            TopologyOperation::Detach { member } => {
                let index = member_index(grid.id(), &candidate, *member)?;
                candidate.remove(index);
            }
            TopologyOperation::Replace {
                member,
                replacement,
            } => {
                let index = member_index(grid.id(), &candidate, *member)?;
                candidate[index] = validate_incoming_member(grid, replacement, kind)?;
            }
        }
    }

    SyncGroupSnapshot::try_new(grid.clone(), revision, candidate)?;
    Ok(())
}

fn validate_incoming_member(
    parent: &BeatGridSnapshot,
    member: &SyncMember<Deck>,
    kind: GroupKind,
) -> Result<SyncMemberSnapshot, SyncError> {
    let expected = match kind {
        GroupKind::Host => SyncMemberKind::Group,
        GroupKind::Deck => SyncMemberKind::Grid,
    };
    let given = member.kind();
    if given != expected {
        return Err(SyncError::InvalidMemberKind {
            group_id: parent.id(),
            member_id: member.id(),
            expected,
            given,
        });
    }
    let snapshot = member.snapshot_for(parent)?;
    if let Some(alignment) = member.alignment() {
        if alignment.target().stamp() != parent.stamp() {
            return Err(SyncGroupTopologyError::StaleTargetAlignment {
                expected: parent.stamp(),
                given: alignment.target().stamp(),
            }
            .into());
        }
        if alignment.source().stamp() != snapshot.grid().stamp() {
            return Err(SyncGroupTopologyError::StaleSourceAlignment {
                expected: snapshot.grid().stamp(),
                given: alignment.source().stamp(),
            }
            .into());
        }
    }
    Ok(snapshot)
}

fn member_index(
    group_id: BeatGridId,
    members: &[SyncMemberSnapshot],
    member_id: BeatGridId,
) -> Result<usize, SyncError> {
    members
        .iter()
        .position(|member| member.grid().id() == member_id)
        .ok_or(SyncError::MemberNotFound {
            group_id,
            member_id,
        })
}

fn apply_topology_operations(
    members: &mut Vec<SyncMember<Deck>>,
    operations: Box<[TopologyOperation<Deck>]>,
) {
    for operation in Vec::from(operations) {
        match operation {
            TopologyOperation::Attach { member } => members.push(member),
            TopologyOperation::Detach { member } => {
                let index = members
                    .iter()
                    .position(|candidate| candidate.id() == member)
                    .expect("invariant: topology preflight validated the detach target");
                members.remove(index);
            }
            TopologyOperation::Replace {
                member,
                replacement,
            } => {
                let index = members
                    .iter()
                    .position(|candidate| candidate.id() == member)
                    .expect("invariant: topology preflight validated the replacement target");
                members[index] = replacement;
            }
        }
    }
}

fn preview_topology(
    topology: &SyncGroupSnapshot,
    base: TopologyStamp,
    operations: &[TopologyOperation<Deck>],
    kind: GroupKind,
) -> Result<(SyncGroupSnapshot, bool), SyncError> {
    if topology.stamp().group_id() == base.group_id() {
        if topology.stamp() != base {
            return Err(SyncError::StaleTopology {
                expected: topology.stamp(),
                given: base,
            });
        }
        if operations.is_empty() {
            return Ok((topology.clone(), false));
        }

        let revision = next_topology_revision(base.group_id(), base.revision())?;
        let mut members = topology.members().to_vec();
        for operation in operations {
            match operation {
                TopologyOperation::Attach { member } => {
                    members.push(validate_incoming_member(
                        topology.group_grid(),
                        member,
                        kind,
                    )?);
                }
                TopologyOperation::Detach { member } => {
                    let index = member_index(base.group_id(), &members, *member)?;
                    members.remove(index);
                }
                TopologyOperation::Replace {
                    member,
                    replacement,
                } => {
                    let index = member_index(base.group_id(), &members, *member)?;
                    members[index] =
                        validate_incoming_member(topology.group_grid(), replacement, kind)?;
                }
            }
        }
        let candidate =
            SyncGroupSnapshot::try_new(topology.group_grid().clone(), revision, members)?;
        return Ok((candidate, true));
    }

    let target = base.group_id();
    let index = topology
        .members()
        .iter()
        .position(|member| {
            member.group_topology().is_some_and(|group| {
                group.stamp().group_id() == target || topology_contains(group, target)
            })
        })
        .ok_or(SyncError::GroupNotFound { group_id: target })?;
    let member = &topology.members()[index];
    let child = member
        .group_topology()
        .ok_or(SyncError::GroupNotFound { group_id: target })?;
    let (child, changed) = preview_topology(child, base, operations, GroupKind::Deck)?;
    if !changed {
        return Ok((topology.clone(), false));
    }

    let revision =
        next_topology_revision(topology.stamp().group_id(), topology.stamp().revision())?;
    let mut members = topology.members().to_vec();
    members[index] = SyncMemberSnapshot::new_group(child, member.alignment());
    let candidate = SyncGroupSnapshot::try_new(topology.group_grid().clone(), revision, members)?;
    Ok((candidate, true))
}

fn owns_direct_grid(members: &[SyncMember<Deck>], target: BeatGridId) -> bool {
    members
        .iter()
        .any(|member| matches!(member, SyncMember::Grid { grid, .. } if grid.id() == target))
}

fn routed_group(
    members: &mut [SyncMember<Deck>],
    target: BeatGridId,
) -> Result<Option<&mut Deck>, SyncError> {
    for member in members {
        let SyncMember::Group { group, .. } = member else {
            continue;
        };
        let group_id = group.id();
        let topology = group.topology()?;
        if topology.stamp().group_id() != group_id {
            return Err(SyncError::GridIdentityMismatch {
                expected: group_id,
                given: topology.stamp().group_id(),
            });
        }
        if group_id == target || topology_contains(&topology, target) {
            return Ok(Some(group.as_mut()));
        }
    }
    Ok(None)
}

fn topology_contains(topology: &SyncGroupSnapshot, target: BeatGridId) -> bool {
    topology.members().iter().any(|member| {
        member.grid().id() == target
            || member
                .group_topology()
                .is_some_and(|group| topology_contains(group, target))
    })
}

fn next_topology_revision(
    group_id: BeatGridId,
    current: TopologyRevision,
) -> Result<TopologyRevision, SyncError> {
    current
        .checked_next()
        .ok_or(SyncError::TopologyRevisionExhausted { group_id })
}

fn take_operation(
    group_id: BeatGridId,
    next: &mut Option<SyncOperationId>,
) -> Result<SyncOperationId, SyncError> {
    let operation = (*next).ok_or(SyncError::OperationIdExhausted { group_id })?;
    advance_operation(next);
    Ok(operation)
}

fn advance_operation(next: &mut Option<SyncOperationId>) {
    *next = next.and_then(SyncOperationId::checked_next);
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;
    use kithara_warp::{AssetAxis, BeatGridUnavailable, SessionAnchor, SessionBeat, SessionFrame};

    use super::*;

    fn session_grid(
        id: BeatGridId,
        revision: BeatGridRevision,
        epoch: SessionEpoch,
        beats_per_second: f64,
    ) -> BeatGridSnapshot {
        session_grid_at_rate(id, revision, epoch, beats_per_second, 48_000)
    }

    fn session_grid_at_rate(
        id: BeatGridId,
        revision: BeatGridRevision,
        epoch: SessionEpoch,
        beats_per_second: f64,
        sample_rate: u32,
    ) -> BeatGridSnapshot {
        let sample_rate =
            NonZeroU32::new(sample_rate).expect("invariant: fixture sample rate is non-zero");
        let anchor = SessionAnchor::new(
            SessionFrame::new(0),
            SessionBeat::new(0.0).expect("invariant: fixture beat is finite"),
            beats_per_second,
            sample_rate,
        )
        .expect("invariant: fixture session anchor is valid");
        BeatGridSnapshot::session(id, revision, epoch, anchor, None)
    }

    fn fixture_host() -> Host {
        Host::new(
            BeatGridId::allocate().expect("invariant: fixture host id is available"),
            NonZeroU32::new(48_000).expect("invariant: fixture sample rate is non-zero"),
        )
    }

    #[kithara::test]
    fn host_rejects_foreign_grid_identity() {
        let mut host = fixture_host();
        let before = host.snapshot();
        let foreign = session_grid(
            BeatGridId::allocate().expect("invariant: foreign grid id is available"),
            before
                .revision()
                .checked_next()
                .expect("invariant: fixture grid revision can advance"),
            SessionEpoch::new(0),
            2.0,
        );

        assert_eq!(
            host.publish_grid(foreign.clone()),
            Err(SyncError::GridIdentityMismatch {
                expected: before.id(),
                given: foreign.id(),
            })
        );
        assert_eq!(host.snapshot(), before);
    }

    #[kithara::test]
    fn host_enforces_grid_successors() {
        let mut host = fixture_host();
        let initial = host.snapshot();
        let published_revision = initial
            .revision()
            .checked_next()
            .expect("invariant: fixture grid revision can advance");
        let published = session_grid(initial.id(), published_revision, SessionEpoch::new(0), 2.0);
        host.publish_grid(published.clone())
            .expect("invariant: newer fixture publication is valid");
        let stale = session_grid(initial.id(), initial.revision(), SessionEpoch::new(0), 1.5);

        assert_eq!(
            host.publish_grid(stale.clone()),
            Err(SyncError::StaleGridRevision {
                current: published.stamp(),
                given: stale.stamp(),
            })
        );
        assert_eq!(host.snapshot(), published);

        let withdrawn_revision = published_revision
            .checked_next()
            .expect("invariant: fixture grid revision can advance twice");
        let withdrawn =
            BeatGridSnapshot::unavailable(initial.id(), withdrawn_revision, published.axis());
        assert_eq!(
            host.publish_grid(withdrawn.clone()),
            Err(SyncError::InvalidGroupGridTransition {
                from: BeatGridState::Live,
                to: BeatGridState::Unavailable(BeatGridUnavailable::NoGeometry),
            })
        );
        assert_eq!(host.snapshot(), published);

        let wrong_axis_revision = published_revision
            .checked_next()
            .expect("invariant: fixture grid revision can advance twice");
        let sample_rate =
            NonZeroU32::new(48_000).expect("invariant: fixture sample rate is non-zero");
        let wrong_axis = BeatGridSnapshot::unavailable(
            initial.id(),
            wrong_axis_revision,
            MapAxis::Asset(AssetAxis::new(sample_rate, 0)),
        );
        assert_eq!(
            host.publish_grid(wrong_axis.clone()),
            Err(SyncError::GridAxisChanged {
                expected: published.axis(),
                given: wrong_axis.axis(),
            })
        );
        assert_eq!(host.snapshot(), published);

        let mut negotiated = fixture_host();
        let initial = negotiated.snapshot();
        let live_revision = initial
            .revision()
            .checked_next()
            .expect("invariant: fixture grid revision can advance");
        let observed = session_grid_at_rate(
            initial.id(),
            live_revision,
            SessionEpoch::new(0),
            2.0,
            44_100,
        );
        negotiated
            .publish_grid(observed.clone())
            .expect("an unavailable axis admits the negotiated live sample rate");
        assert_eq!(negotiated.snapshot(), observed);

        let changed_rate = session_grid_at_rate(
            initial.id(),
            live_revision
                .checked_next()
                .expect("invariant: fixture grid revision can advance twice"),
            SessionEpoch::new(0),
            2.0,
            32_000,
        );
        assert_eq!(
            negotiated.publish_grid(changed_rate.clone()),
            Err(SyncError::GridAxisChanged {
                expected: observed.axis(),
                given: changed_rate.axis(),
            })
        );
        assert_eq!(negotiated.snapshot(), observed);
    }

    #[kithara::test]
    fn host_treats_same_grid_stamp_as_idempotent_publication() {
        let mut host = fixture_host();
        let current = host.snapshot();
        let revision = current
            .revision()
            .checked_next()
            .expect("invariant: fixture grid revision can advance");
        let published = session_grid(current.id(), revision, SessionEpoch::new(0), 2.0);
        host.publish_grid(published.clone())
            .expect("invariant: newer fixture publication is valid");

        host.publish_grid(published.clone())
            .expect("publishing the same immutable grid revision is idempotent");

        assert_eq!(host.snapshot(), published);
    }

    #[kithara::test]
    fn host_accepts_latest_grid_after_unpublished_revisions() {
        let mut host = fixture_host();
        let current = host.snapshot();
        let skipped = current
            .revision()
            .checked_next()
            .expect("invariant: fixture grid revision can advance");
        let published_revision = skipped
            .checked_next()
            .expect("invariant: fixture grid revision can advance twice");
        let published = session_grid(current.id(), published_revision, SessionEpoch::new(0), 2.0);

        host.publish_grid(published.clone())
            .expect("a newer published observation may skip invisible revisions");

        assert_eq!(host.snapshot(), published);
        assert_eq!(host.snapshot().revision(), published_revision);
    }

    #[kithara::test]
    fn host_requires_each_unavailable_route_boundary() {
        let mut host = fixture_host();
        let initial = host.snapshot();
        let live_revision = initial
            .revision()
            .checked_next()
            .expect("invariant: fixture grid revision can advance");
        let live = session_grid(initial.id(), live_revision, SessionEpoch::new(0), 2.0);
        host.publish_grid(live.clone())
            .expect("the initial session grid becomes live in its current epoch");

        let boundary_revision = live_revision
            .checked_next()
            .expect("invariant: fixture grid revision can advance twice");
        let sample_rate =
            NonZeroU32::new(48_000).expect("invariant: fixture sample rate is non-zero");
        let skipped_axis = MapAxis::Session(SessionAxis::new(sample_rate, SessionEpoch::new(2)));
        let skipped = BeatGridSnapshot::unavailable(live.id(), boundary_revision, skipped_axis);
        assert_eq!(
            host.publish_grid(skipped),
            Err(SyncError::GridAxisChanged {
                expected: live.axis(),
                given: skipped_axis,
            })
        );

        let successor_live = session_grid(live.id(), boundary_revision, SessionEpoch::new(1), 2.0);
        assert_eq!(
            host.publish_grid(successor_live.clone()),
            Err(SyncError::GridAxisChanged {
                expected: live.axis(),
                given: successor_live.axis(),
            })
        );

        let boundary_axis = MapAxis::Session(SessionAxis::new(sample_rate, SessionEpoch::new(1)));
        let boundary = BeatGridSnapshot::unavailable(live.id(), boundary_revision, boundary_axis);
        host.publish_grid(boundary.clone())
            .expect("the exact successor epoch is admitted through an unavailable boundary");

        let next_live = session_grid_at_rate(
            live.id(),
            boundary_revision
                .checked_next()
                .expect("invariant: fixture grid revision can advance three times"),
            SessionEpoch::new(1),
            2.0,
            44_100,
        );
        host.publish_grid(next_live.clone())
            .expect("the unavailable boundary admits the negotiated live axis");
        assert_eq!(host.snapshot(), next_live);
    }
}
