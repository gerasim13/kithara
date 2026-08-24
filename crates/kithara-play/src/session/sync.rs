use std::num::NonZeroU32;

use kithara_audio::{
    AlignmentPlan, AlignmentRequest, BeatMap, BeatMapId, BeatMapRevision, BeatMapSnapshot,
    HostAxis, HostEpoch, MapAxis, MapStamp, PlanTransition, PresentationFrontier, SyncAdmission,
    SyncApplied, SyncCapability, SyncError, SyncGroup, SyncGroupSnapshot, SyncGroupTopologyError,
    SyncIntent, SyncMember, SyncMemberKind, SyncMemberSnapshot, SyncOperation, SyncOperationId,
    SyncRejected, SyncStatusSnapshot, TopologyOperation, TopologyRevision, TopologyStamp,
};

use super::state::Deck;

pub(super) struct Host {
    map: BeatMapSnapshot,
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
    pub(super) fn new(id: BeatMapId, sample_rate: NonZeroU32) -> Self {
        Self {
            map: unavailable_map(id, BeatMapRevision::first(), sample_rate, HostEpoch::new(0)),
            decks: Vec::new(),
            next_operation: Some(SyncOperationId::first()),
            topology_revision: TopologyRevision::first(),
            unavailable: None,
        }
    }

    pub(super) fn publish_map(&mut self, map: BeatMapSnapshot) -> Result<(), SyncError> {
        publish_map(&mut self.map, map)
    }

    pub(super) fn publish_unavailable(
        &mut self,
        stamp: MapStamp,
        sample_rate: NonZeroU32,
        epoch: HostEpoch,
    ) -> Result<(), SyncError> {
        self.publish_map(unavailable_map(
            stamp.map_id(),
            stamp.revision(),
            sample_rate,
            epoch,
        ))
    }

    pub(super) fn deck_count(&self) -> usize {
        self.decks.len()
    }

    pub(super) fn deck(&self, index: usize) -> Option<&Deck> {
        match self.decks.get(index)? {
            SyncMember::Group { group, .. } => Some(group),
            SyncMember::Map { .. } => None,
        }
    }

    pub(super) fn deck_mut(&mut self, index: usize) -> Option<&mut Deck> {
        match self.decks.get_mut(index)? {
            SyncMember::Group { group, .. } => Some(group),
            SyncMember::Map { .. } => None,
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
            SyncMember::Map { .. } => None,
        })
    }
}

pub(super) fn unavailable_map(
    id: BeatMapId,
    revision: BeatMapRevision,
    sample_rate: NonZeroU32,
    epoch: HostEpoch,
) -> BeatMapSnapshot {
    BeatMapSnapshot::unavailable(
        id,
        revision,
        MapAxis::Host(HostAxis::new(sample_rate, epoch)),
    )
}

fn publish_map(current: &mut BeatMapSnapshot, next: BeatMapSnapshot) -> Result<(), SyncError> {
    if next.id() != current.id() {
        return Err(SyncError::MapIdentityMismatch {
            expected: current.id(),
            given: next.id(),
        });
    }
    if next == *current {
        return Ok(());
    }
    if next.revision() <= current.revision() {
        return Err(SyncError::StaleMapRevision {
            current: current.stamp(),
            given: next.stamp(),
        });
    }
    *current = next;
    Ok(())
}

impl BeatMap for Host {
    delegate::delegate! {
        to self.map {
            fn id(&self) -> BeatMapId;
            #[call(clone)]
            fn snapshot(&self) -> BeatMapSnapshot;
            fn align_to(
                &self,
                target: &dyn BeatMap,
                request: AlignmentRequest,
            ) -> Result<AlignmentPlan, SyncError>;
            fn reconcile_to(
                &self,
                target: &dyn BeatMap,
                active: &AlignmentPlan,
                frontier: PresentationFrontier,
            ) -> Result<PlanTransition, SyncError>;
        }
    }
}

impl BeatMap for Deck {
    delegate::delegate! {
        to self.map {
            fn id(&self) -> BeatMapId;
            #[call(clone)]
            fn snapshot(&self) -> BeatMapSnapshot;
            fn align_to(
                &self,
                target: &dyn BeatMap,
                request: AlignmentRequest,
            ) -> Result<AlignmentPlan, SyncError>;
            fn reconcile_to(
                &self,
                target: &dyn BeatMap,
                active: &AlignmentPlan,
                frontier: PresentationFrontier,
            ) -> Result<PlanTransition, SyncError>;
        }
    }
}

impl SyncGroup for Host {
    type NestedGroup = Deck;

    fn topology(&self) -> Result<SyncGroupSnapshot, SyncError> {
        materialize_topology(&self.map, self.topology_revision, &self.decks)
    }

    fn transact(
        &mut self,
        operation: SyncOperation<Deck>,
    ) -> Result<SyncAdmission, SyncRejected<Deck>> {
        transact(
            &self.map,
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
            TopologyStamp::new(self.map.id(), self.topology_revision),
            self.unavailable,
        )
    }

    fn acknowledge(&mut self, _applied: SyncApplied) -> Result<(), SyncError> {
        Err(SyncError::NoPreparedOperation)
    }
}

impl SyncGroup for Deck {
    type NestedGroup = Self;

    fn topology(&self) -> Result<SyncGroupSnapshot, SyncError> {
        materialize_topology(&self.map, self.topology_revision, &self.tracks)
    }

    fn transact(
        &mut self,
        operation: SyncOperation<Self>,
    ) -> Result<SyncAdmission, SyncRejected<Self>> {
        transact(
            &self.map,
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
            TopologyStamp::new(self.map.id(), self.topology_revision),
            self.unavailable,
        )
    }

    fn acknowledge(&mut self, _applied: SyncApplied) -> Result<(), SyncError> {
        Err(SyncError::NoPreparedOperation)
    }
}

fn materialize_topology(
    map: &BeatMapSnapshot,
    revision: TopologyRevision,
    members: &[SyncMember<Deck>],
) -> Result<SyncGroupSnapshot, SyncError> {
    let members = members
        .iter()
        .map(|member| member.snapshot_for(map))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SyncGroupSnapshot::try_new(map.clone(), revision, members)?)
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
    map: &BeatMapSnapshot,
    topology_revision: &mut TopologyRevision,
    members: &mut Vec<SyncMember<Deck>>,
    next_operation: &mut Option<SyncOperationId>,
    unavailable: &mut Option<(SyncOperationId, SyncCapability)>,
    kind: GroupKind,
    operation: SyncOperation<Deck>,
) -> Result<SyncAdmission, SyncRejected<Deck>> {
    let target = operation.target();
    let topology_operation = matches!(&operation, SyncOperation::Topology { .. });
    if target == map.id() || (!topology_operation && owns_direct_map(members, target)) {
        return transact_local(
            map,
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
        let root = match materialize_topology(map, *topology_revision, members) {
            Ok(root) => root,
            Err(error) => return Err(SyncRejected::new(error, operation)),
        };
        if let Err(error) = preview_topology(&root, *base, operations, kind) {
            return Err(SyncRejected::new(error, operation));
        }
    }
    let parent_revision = match topology_change
        .then(|| next_topology_revision(map.id(), *topology_revision))
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
    map: &BeatMapSnapshot,
    topology_revision: &mut TopologyRevision,
    members: &mut Vec<SyncMember<Deck>>,
    next_operation: &mut Option<SyncOperationId>,
    unavailable: &mut Option<(SyncOperationId, SyncCapability)>,
    kind: GroupKind,
    operation: SyncOperation<Deck>,
) -> Result<SyncAdmission, SyncRejected<Deck>> {
    match &operation {
        SyncOperation::Topology { .. } => transact_topology(
            map,
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
            let operation_id = match take_operation(map.id(), next_operation) {
                Ok(operation_id) => operation_id,
                Err(error) => return Err(SyncRejected::new(error, operation)),
            };
            *unavailable = None;
            Ok(SyncAdmission::Unchanged {
                operation: operation_id,
                topology: TopologyStamp::new(map.id(), *topology_revision),
            })
        }
        SyncOperation::Transport {
            load, transport, ..
        } => {
            let load = *load;
            let transport = *transport;
            let operation_id = match take_operation(map.id(), next_operation) {
                Ok(operation_id) => operation_id,
                Err(error) => return Err(SyncRejected::new(error, operation)),
            };
            *unavailable = None;
            Ok(SyncAdmission::Accepted {
                operation: operation_id,
                topology: TopologyStamp::new(map.id(), *topology_revision),
                load,
                transport,
            })
        }
        SyncOperation::Sync { .. } => preserve_rejected(
            unavailable_admission(
                map.id(),
                *topology_revision,
                next_operation,
                unavailable,
                SyncCapability::Alignment,
            ),
            operation,
        ),
        SyncOperation::Reconcile { .. } => preserve_rejected(
            unavailable_admission(
                map.id(),
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
    group_id: BeatMapId,
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
    map: &BeatMapSnapshot,
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
    let expected = TopologyStamp::new(map.id(), *topology_revision);
    if base != expected {
        return Err(reject(
            SyncError::StaleTopology {
                expected,
                given: base,
            },
            operations,
        ));
    }

    let operation_id = match (*next_operation)
        .ok_or_else(|| SyncError::OperationIdExhausted { group_id: map.id() })
    {
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

    let revision = match next_topology_revision(map.id(), *topology_revision) {
        Ok(revision) => revision,
        Err(error) => return Err(reject(error, operations)),
    };
    if let Err(error) = validate_topology_candidate(map, revision, members, &operations, kind) {
        return Err(reject(error, operations));
    }
    apply_topology_operations(members, operations);
    *topology_revision = revision;
    advance_operation(next_operation);
    Ok(SyncAdmission::TopologyChanged {
        operation: operation_id,
        topology: TopologyStamp::new(map.id(), revision),
    })
}

fn validate_topology_candidate(
    map: &BeatMapSnapshot,
    revision: TopologyRevision,
    members: &[SyncMember<Deck>],
    operations: &[TopologyOperation<Deck>],
    kind: GroupKind,
) -> Result<(), SyncError> {
    let mut candidate: Vec<SyncMemberSnapshot> = members
        .iter()
        .map(|member| member.snapshot_for(map))
        .collect::<Result<_, _>>()?;

    for operation in operations {
        match operation {
            TopologyOperation::Attach { member } => {
                let snapshot = validate_incoming_member(map, member, kind)?;
                candidate.push(snapshot);
            }
            TopologyOperation::Detach { member } => {
                let index = member_index(map.id(), &candidate, *member)?;
                candidate.remove(index);
            }
            TopologyOperation::Replace {
                member,
                replacement,
            } => {
                let index = member_index(map.id(), &candidate, *member)?;
                candidate[index] = validate_incoming_member(map, replacement, kind)?;
            }
        }
    }

    SyncGroupSnapshot::try_new(map.clone(), revision, candidate)?;
    Ok(())
}

fn validate_incoming_member(
    parent: &BeatMapSnapshot,
    member: &SyncMember<Deck>,
    kind: GroupKind,
) -> Result<SyncMemberSnapshot, SyncError> {
    let expected = match kind {
        GroupKind::Host => SyncMemberKind::Group,
        GroupKind::Deck => SyncMemberKind::Map,
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
        if alignment.source().stamp() != snapshot.map().stamp() {
            return Err(SyncGroupTopologyError::StaleSourceAlignment {
                expected: snapshot.map().stamp(),
                given: alignment.source().stamp(),
            }
            .into());
        }
    }
    Ok(snapshot)
}

fn member_index(
    group_id: BeatMapId,
    members: &[SyncMemberSnapshot],
    member_id: BeatMapId,
) -> Result<usize, SyncError> {
    members
        .iter()
        .position(|member| member.map().id() == member_id)
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
                        topology.group_map(),
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
                        validate_incoming_member(topology.group_map(), replacement, kind)?;
                }
            }
        }
        let candidate =
            SyncGroupSnapshot::try_new(topology.group_map().clone(), revision, members)?;
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
    let candidate = SyncGroupSnapshot::try_new(topology.group_map().clone(), revision, members)?;
    Ok((candidate, true))
}

fn owns_direct_map(members: &[SyncMember<Deck>], target: BeatMapId) -> bool {
    members
        .iter()
        .any(|member| matches!(member, SyncMember::Map { map, .. } if map.id() == target))
}

fn routed_group(
    members: &mut [SyncMember<Deck>],
    target: BeatMapId,
) -> Result<Option<&mut Deck>, SyncError> {
    for member in members {
        let SyncMember::Group { group, .. } = member else {
            continue;
        };
        let group_id = group.id();
        let topology = group.topology()?;
        if topology.stamp().group_id() != group_id {
            return Err(SyncError::MapIdentityMismatch {
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

fn topology_contains(topology: &SyncGroupSnapshot, target: BeatMapId) -> bool {
    topology.members().iter().any(|member| {
        member.map().id() == target
            || member
                .group_topology()
                .is_some_and(|group| topology_contains(group, target))
    })
}

fn next_topology_revision(
    group_id: BeatMapId,
    current: TopologyRevision,
) -> Result<TopologyRevision, SyncError> {
    current
        .checked_next()
        .ok_or(SyncError::TopologyRevisionExhausted { group_id })
}

fn take_operation(
    group_id: BeatMapId,
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
mod tests;
