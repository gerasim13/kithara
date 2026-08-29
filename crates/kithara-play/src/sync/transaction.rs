use kithara_warp::{
    BeatGridId, BeatGridSnapshot, SyncAdmission, SyncCapability, SyncError, SyncGroup, SyncMember,
    SyncMemberKind, SyncOperation, SyncOperationId, SyncRejected, SyncStatusSnapshot,
    TopologyRevision, TopologyStamp,
};

use super::topology::{
    apply_topology_operations, materialize_topology, next_topology_revision, owns_direct_grid,
    preview_topology, routed_group, validate_topology_candidate,
};

pub(super) fn status(
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

pub(super) fn transact<G: SyncGroup<NestedGroup = G>>(
    grid: &BeatGridSnapshot,
    topology_revision: &mut TopologyRevision,
    members: &mut Vec<SyncMember<G>>,
    next_operation: &mut Option<SyncOperationId>,
    unavailable: &mut Option<(SyncOperationId, SyncCapability)>,
    member_kind: SyncMemberKind,
    operation: SyncOperation<G>,
) -> Result<SyncAdmission, SyncRejected<G>> {
    let target = operation.target();
    let topology_operation = matches!(&operation, SyncOperation::Topology { .. });
    if target == grid.id() || (!topology_operation && owns_direct_grid(members, target)) {
        return transact_local(
            grid,
            topology_revision,
            members,
            next_operation,
            unavailable,
            member_kind,
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
        if let Err(error) = preview_topology(&root, *base, operations, member_kind) {
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

fn transact_local<G: SyncGroup<NestedGroup = G>>(
    grid: &BeatGridSnapshot,
    topology_revision: &mut TopologyRevision,
    members: &mut Vec<SyncMember<G>>,
    next_operation: &mut Option<SyncOperationId>,
    unavailable: &mut Option<(SyncOperationId, SyncCapability)>,
    member_kind: SyncMemberKind,
    operation: SyncOperation<G>,
) -> Result<SyncAdmission, SyncRejected<G>> {
    match &operation {
        SyncOperation::Topology { .. } => transact_topology(
            grid,
            topology_revision,
            members,
            next_operation,
            member_kind,
            operation,
        ),
        SyncOperation::Sync {
            intent: kithara_warp::SyncIntent::Disable,
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

fn preserve_rejected<G: SyncGroup<NestedGroup = G>>(
    result: Result<SyncAdmission, SyncError>,
    operation: SyncOperation<G>,
) -> Result<SyncAdmission, SyncRejected<G>> {
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

fn transact_topology<G: SyncGroup<NestedGroup = G>>(
    grid: &BeatGridSnapshot,
    topology_revision: &mut TopologyRevision,
    members: &mut Vec<SyncMember<G>>,
    next_operation: &mut Option<SyncOperationId>,
    member_kind: SyncMemberKind,
    operation: SyncOperation<G>,
) -> Result<SyncAdmission, SyncRejected<G>> {
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
    if let Err(error) =
        validate_topology_candidate(grid, revision, members, &operations, member_kind)
    {
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
