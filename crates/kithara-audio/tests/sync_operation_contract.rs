use std::num::NonZeroU32;

use kithara_audio::{
    AlignmentPlan, AlignmentRequest, AlignmentSource, AssetBeatMap, Beat, BeatAlignment, BeatMap,
    BeatMapId, BeatMapRevision, BeatMapSnapshot, HostBeatMap, HostEpoch, LoadGeneration, MapPoint,
    PlanTransition, PresentationFrontier, ReconcileCause, SessionAnchor, SessionBeat, SessionFrame,
    SyncAdmission, SyncApplied, SyncCapability, SyncError, SyncGroup, SyncGroupSnapshot,
    SyncGroupTopologyError, SyncIntent, SyncMember, SyncMemberKind, SyncOperation, SyncOperationId,
    SyncRejected, SyncStatusSnapshot, TopologyOperation, TopologyRevision, TopologyStamp,
    TransportOperation, TransportRevision,
};
use kithara_test_utils::kithara;

fn map_id() -> BeatMapId {
    BeatMapId::allocate().expect("invariant: test map identity space is available")
}

fn sample_rate() -> NonZeroU32 {
    NonZeroU32::new(48_000).expect("invariant: sample rate is non-zero")
}

fn host_map() -> HostBeatMap {
    let anchor = SessionAnchor::new(
        SessionFrame::new(0),
        SessionBeat::new(0.0).expect("invariant: host beat is finite"),
        2.0,
        sample_rate(),
    )
    .expect("invariant: host relation is valid");
    HostBeatMap::new(
        map_id(),
        BeatMapRevision::first(),
        HostEpoch::new(1),
        anchor,
        None,
    )
}

#[derive(Debug)]
struct ContractGroup(SyncGroupSnapshot);

impl BeatMap for ContractGroup {
    fn id(&self) -> BeatMapId {
        self.0.group_map().id()
    }

    fn snapshot(&self) -> BeatMapSnapshot {
        self.0.group_map().clone()
    }

    fn align_to(
        &self,
        target: &dyn BeatMap,
        request: AlignmentRequest,
    ) -> Result<AlignmentPlan, SyncError> {
        self.0.group_map().align_to(target, request)
    }

    fn reconcile_to(
        &self,
        target: &dyn BeatMap,
        active: &AlignmentPlan,
        frontier: PresentationFrontier,
    ) -> Result<PlanTransition, SyncError> {
        self.0.group_map().reconcile_to(target, active, frontier)
    }
}

impl SyncGroup for ContractGroup {
    type NestedGroup = Self;

    fn topology(&self) -> Result<SyncGroupSnapshot, SyncError> {
        Ok(self.0.clone())
    }

    fn transact(
        &mut self,
        operation: SyncOperation<Self::NestedGroup>,
    ) -> Result<SyncAdmission, SyncRejected<Self::NestedGroup>> {
        Err(SyncRejected::new(
            SyncError::CapabilityUnavailable {
                capability: SyncCapability::Topology,
            },
            operation,
        ))
    }

    fn status(&self) -> SyncStatusSnapshot {
        SyncStatusSnapshot::Off {
            topology: self.0.stamp(),
        }
    }

    fn acknowledge(&mut self, _applied: SyncApplied) -> Result<(), SyncError> {
        Err(SyncError::NoPreparedOperation)
    }
}

fn zero(map: &BeatMapSnapshot) -> MapPoint<Beat> {
    MapPoint::new(
        map.stamp(),
        Beat::new(0.0).expect("invariant: zero beat is finite"),
    )
}

fn member(parent: &dyn BeatMap) -> SyncMember<ContractGroup> {
    let parent = parent.snapshot();
    let (map, _publisher) = AssetBeatMap::new(map_id(), sample_rate(), 48_001);
    let source = map.snapshot();
    SyncMember::Map {
        alignment: Some(BeatAlignment::new(zero(&source), zero(&parent))),
        map: Box::new(map),
    }
}

fn frontier() -> PresentationFrontier {
    PresentationFrontier::builder()
        .source(24_000)
        .output(SessionFrame::new(24_000))
        .build()
}

#[kithara::test]
fn topology_shape_has_one_base_and_parent_free_edits() {
    let host = host_map();
    let base = TopologyStamp::new(host.id(), TopologyRevision::first());
    let attached = member(&host);
    let attached_id = attached.id();
    let replacement = member(&host);
    let replacement_id = replacement.id();

    assert_eq!(attached.kind(), SyncMemberKind::Map);
    assert_eq!(replacement.kind(), SyncMemberKind::Map);

    let operation = SyncOperation::Topology {
        base,
        operations: Box::new([
            TopologyOperation::Attach { member: attached },
            TopologyOperation::Detach {
                member: attached_id,
            },
            TopologyOperation::Replace {
                member: attached_id,
                replacement,
            },
        ]),
    };

    assert_eq!(operation.target(), base.group_id());

    let SyncOperation::Topology {
        base: observed_base,
        operations,
    } = operation
    else {
        panic!("expected one topology transaction");
    };
    assert_eq!(observed_base, base);
    let [attach, detach, replace] = operations.as_ref() else {
        panic!("expected all topology operation shapes");
    };
    match attach {
        TopologyOperation::Attach { member } => assert_eq!(member.id(), attached_id),
        _ => panic!("expected attach"),
    }
    match detach {
        TopologyOperation::Detach { member } => assert_eq!(*member, attached_id),
        _ => panic!("expected detach"),
    }
    match replace {
        TopologyOperation::Replace {
            member,
            replacement,
        } => {
            assert_eq!(*member, attached_id);
            assert_eq!(replacement.id(), replacement_id);
        }
        _ => panic!("expected replace"),
    }
}

#[kithara::test]
fn topology_admission_exposes_operation_and_resulting_stamp() {
    let topology = TopologyStamp::new(map_id(), TopologyRevision::first());
    let operation = SyncOperationId::first();
    let admission = SyncAdmission::TopologyChanged {
        operation,
        topology,
    };

    match admission {
        SyncAdmission::TopologyChanged {
            operation: observed_operation,
            topology: observed_topology,
        } => {
            assert_eq!(observed_operation, operation);
            assert_eq!(observed_topology, topology);
        }
        _ => panic!("expected topology admission"),
    }
}

#[kithara::test]
fn rejected_topology_returns_the_live_member_to_its_caller() {
    let host = host_map();
    let topology = SyncGroupSnapshot::try_new(
        host.snapshot(),
        TopologyRevision::first(),
        std::iter::empty(),
    )
    .expect("invariant: an empty contract group is valid");
    let mut group = ContractGroup(topology);
    let attached = member(&group);
    let attached_id = attached.id();
    let operation = SyncOperation::Topology {
        base: group.topology().expect("group is observable").stamp(),
        operations: Box::new([TopologyOperation::Attach { member: attached }]),
    };

    let rejection = group
        .transact(operation)
        .expect_err("the frozen group rejects topology transactions");
    let (error, operation): (SyncError, SyncOperation<ContractGroup>) = rejection.into();

    assert_eq!(
        error,
        SyncError::CapabilityUnavailable {
            capability: SyncCapability::Topology,
        }
    );
    let SyncOperation::Topology { operations, .. } = operation else {
        panic!("expected the rejected topology transaction");
    };
    let [TopologyOperation::Attach { member }] = operations.as_ref() else {
        panic!("expected the rejected live member");
    };
    assert_eq!(member.id(), attached_id);
}

#[kithara::test]
fn accepted_transport_preserves_every_dispatch_stamp() {
    let operation = SyncOperationId::first();
    let topology = TopologyStamp::new(map_id(), TopologyRevision::first());
    let load = LoadGeneration::first();
    let transport = TransportRevision::first();
    let admission = SyncAdmission::Accepted {
        operation,
        topology,
        load,
        transport,
    };

    match admission {
        SyncAdmission::Accepted {
            operation: observed_operation,
            topology: observed_topology,
            load: observed_load,
            transport: observed_transport,
        } => {
            assert_eq!(observed_operation, operation);
            assert_eq!(observed_topology, topology);
            assert_eq!(observed_load, load);
            assert_eq!(observed_transport, transport);
        }
        _ => panic!("expected accepted transport admission"),
    }
}

fn observe_transport_operation(operation: &TransportOperation) {
    match operation {
        TransportOperation::PrepareStart { source_frame }
        | TransportOperation::Seek { source_frame } => {
            let _: &u64 = source_frame;
        }
        TransportOperation::Play | TransportOperation::Pause | TransportOperation::Stop => {}
        _ => {}
    }
}

#[kithara::test]
fn transport_shape_exposes_each_operation_and_target() {
    let target = host_map().id();
    let load = LoadGeneration::first();
    let transport = TransportRevision::first();
    let prepared_source = 24_000;
    let seek_source = 96_000;

    for transport_operation in [
        TransportOperation::PrepareStart {
            source_frame: prepared_source,
        },
        TransportOperation::Play,
        TransportOperation::Pause,
        TransportOperation::Seek {
            source_frame: seek_source,
        },
        TransportOperation::Stop,
    ] {
        let operation = SyncOperation::<ContractGroup>::Transport {
            target,
            load,
            transport,
            operation: transport_operation,
        };

        assert_eq!(operation.target(), target);
        match &operation {
            SyncOperation::Transport {
                target: observed_target,
                load: observed_load,
                transport: observed_transport,
                operation,
            } => {
                assert_eq!(*observed_target, target);
                assert_eq!(*observed_load, load);
                assert_eq!(*observed_transport, transport);
                observe_transport_operation(operation);
                match operation {
                    TransportOperation::PrepareStart { source_frame } => {
                        assert_eq!(*source_frame, prepared_source);
                    }
                    TransportOperation::Seek { source_frame } => {
                        assert_eq!(*source_frame, seek_source);
                    }
                    _ => {}
                }
            }
            _ => panic!("expected transport operation"),
        }
    }
}

fn observe_sync_intent(intent: &SyncIntent) {
    match intent {
        SyncIntent::Enable | SyncIntent::Disable | SyncIntent::AlignNow => {}
        _ => {}
    }
}

#[kithara::test]
fn sync_shape_exposes_each_intent_and_target() {
    let target = host_map().id();
    let load = LoadGeneration::first();
    let transport = TransportRevision::first();
    let source = AlignmentSource::Prepared;
    let activation = SessionFrame::new(24_000);

    for intent in [
        SyncIntent::Enable,
        SyncIntent::Disable,
        SyncIntent::AlignNow,
    ] {
        let operation = SyncOperation::<ContractGroup>::Sync {
            target,
            load,
            transport,
            source,
            activation,
            intent,
        };

        assert_eq!(operation.target(), target);
        match &operation {
            SyncOperation::Sync {
                target: observed_target,
                load: observed_load,
                transport: observed_transport,
                source: observed_source,
                activation: observed_activation,
                intent,
            } => {
                assert_eq!(*observed_target, target);
                assert_eq!(*observed_load, load);
                assert_eq!(*observed_transport, transport);
                assert_eq!(*observed_source, source);
                assert_eq!(*observed_activation, activation);
                observe_sync_intent(intent);
            }
            _ => panic!("expected sync operation"),
        }
    }
}

fn observe_reconcile_cause(cause: &ReconcileCause) {
    match cause {
        ReconcileCause::MapAvailable
        | ReconcileCause::MapRefined
        | ReconcileCause::TransportChanged
        | ReconcileCause::TopologyChanged => {}
        _ => {}
    }
}

#[kithara::test]
fn reconcile_shape_exposes_each_cause_and_target() {
    let target = host_map().id();
    let load = LoadGeneration::first();
    let transport = TransportRevision::first();
    let frontier = frontier();

    for cause in [
        ReconcileCause::MapAvailable,
        ReconcileCause::MapRefined,
        ReconcileCause::TransportChanged,
        ReconcileCause::TopologyChanged,
    ] {
        let operation = SyncOperation::<ContractGroup>::Reconcile {
            target,
            load,
            transport,
            cause,
            frontier,
        };

        assert_eq!(operation.target(), target);
        match &operation {
            SyncOperation::Reconcile {
                target: observed_target,
                load: observed_load,
                transport: observed_transport,
                cause,
                frontier: observed_frontier,
            } => {
                assert_eq!(*observed_target, target);
                assert_eq!(*observed_load, load);
                assert_eq!(*observed_transport, transport);
                assert_eq!(*observed_frontier, frontier);
                observe_reconcile_cause(cause);
            }
            _ => panic!("expected reconcile operation"),
        }
    }
}

fn observe_sync_error(error: &SyncError) {
    match error {
        SyncError::StaleTopology { expected, given } => {
            let _: (&TopologyStamp, &TopologyStamp) = (expected, given);
        }
        SyncError::GroupNotFound { group_id }
        | SyncError::TopologyRevisionExhausted { group_id }
        | SyncError::OperationIdExhausted { group_id } => {
            let _: &BeatMapId = group_id;
        }
        SyncError::NoPreparedOperation => {}
        SyncError::MemberNotFound {
            group_id,
            member_id,
        } => {
            let _: (&BeatMapId, &BeatMapId) = (group_id, member_id);
        }
        SyncError::InvalidMemberKind {
            group_id,
            member_id,
            expected,
            given,
        } => {
            let _: (&BeatMapId, &BeatMapId, &SyncMemberKind, &SyncMemberKind) =
                (group_id, member_id, expected, given);
        }
        SyncError::DuplicateAcknowledgement { operation } => {
            let _: &SyncOperationId = operation;
        }
        SyncError::StaleAcknowledgement { expected, given } => {
            let _: (&SyncOperationId, &SyncOperationId) = (expected, given);
        }
        SyncError::AppliedMismatch { expected, given } => {
            let _: (&SyncApplied, &SyncApplied) = (expected, given);
        }
        SyncError::Topology(error) => {
            let _: &SyncGroupTopologyError = error;
        }
        _ => {}
    }
}

#[kithara::test]
fn sync_errors_expose_typed_failure_context() {
    let _contract: fn(&SyncError) = observe_sync_error;
}

#[kithara::test]
fn group_scoped_errors_preserve_group_identity() {
    let group_id = map_id();
    let errors = [
        SyncError::GroupNotFound { group_id },
        SyncError::TopologyRevisionExhausted { group_id },
        SyncError::OperationIdExhausted { group_id },
    ];

    for error in errors {
        let observed = match error {
            SyncError::GroupNotFound { group_id }
            | SyncError::TopologyRevisionExhausted { group_id }
            | SyncError::OperationIdExhausted { group_id } => group_id,
            _ => panic!("expected a group-scoped synchronization error"),
        };
        assert_eq!(observed, group_id);
    }
}

#[kithara::test]
fn invalid_member_kind_preserves_group_member_and_policy() {
    let group_id = map_id();
    let member_id = map_id();
    let error = SyncError::InvalidMemberKind {
        group_id,
        member_id,
        expected: SyncMemberKind::Group,
        given: SyncMemberKind::Map,
    };

    assert_eq!(
        error,
        SyncError::InvalidMemberKind {
            group_id,
            member_id,
            expected: SyncMemberKind::Group,
            given: SyncMemberKind::Map,
        }
    );
}
