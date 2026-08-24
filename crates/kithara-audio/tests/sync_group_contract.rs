use std::num::NonZeroU32;

use kithara_audio::{
    AlignmentPlan, AlignmentRequest, AssetBeatMap, AssetMapUpdate, Beat, BeatAlignment, BeatMap,
    BeatMapId, BeatMapRevision, BeatMapSnapshot, HostBeatMap, HostEpoch, MapAxis, MapPoint,
    MapState, PlanTransition, PresentationFrontier, SessionAnchor, SessionBeat, SessionFrame,
    SyncAdmission, SyncApplied, SyncCapability, SyncError, SyncGroup, SyncGroupSnapshot,
    SyncGroupTopologyError, SyncMember, SyncMemberKind, SyncMemberSnapshot, SyncOperation,
    SyncRejected, SyncStatusSnapshot, TopologyOperation, TopologyRevision,
};
use kithara_test_utils::kithara;

fn map_id() -> BeatMapId {
    BeatMapId::allocate().expect("invariant: test map identity space is available")
}

fn host_map_at(id: BeatMapId, revision: BeatMapRevision) -> HostBeatMap {
    let sample_rate = NonZeroU32::new(48_000).expect("invariant: sample rate is non-zero");
    let anchor = SessionAnchor::new(
        SessionFrame::new(0),
        SessionBeat::new(0.0).expect("invariant: host beat is finite"),
        2.0,
        sample_rate,
    )
    .expect("invariant: host relation is valid");
    HostBeatMap::new(id, revision, HostEpoch::new(1), anchor, None)
}

fn host_map() -> HostBeatMap {
    host_map_at(map_id(), BeatMapRevision::first())
}

fn host_snapshot(id: BeatMapId, revision: BeatMapRevision) -> BeatMapSnapshot {
    host_map_at(id, revision).snapshot()
}

#[derive(Debug)]
struct FrozenMap(BeatMapSnapshot);

impl BeatMap for FrozenMap {
    fn id(&self) -> BeatMapId {
        self.0.id()
    }

    fn snapshot(&self) -> BeatMapSnapshot {
        self.0.clone()
    }

    fn align_to(
        &self,
        target: &dyn BeatMap,
        request: AlignmentRequest,
    ) -> Result<AlignmentPlan, SyncError> {
        self.0.align_to(target, request)
    }

    fn reconcile_to(
        &self,
        target: &dyn BeatMap,
        active: &AlignmentPlan,
        frontier: PresentationFrontier,
    ) -> Result<PlanTransition, SyncError> {
        self.0.reconcile_to(target, active, frontier)
    }
}

#[derive(Debug)]
struct MismatchedMap {
    id: BeatMapId,
    snapshot: BeatMapSnapshot,
}

impl BeatMap for MismatchedMap {
    fn id(&self) -> BeatMapId {
        self.id
    }

    fn snapshot(&self) -> BeatMapSnapshot {
        self.snapshot.clone()
    }

    fn align_to(
        &self,
        target: &dyn BeatMap,
        request: AlignmentRequest,
    ) -> Result<AlignmentPlan, SyncError> {
        self.snapshot.align_to(target, request)
    }

    fn reconcile_to(
        &self,
        target: &dyn BeatMap,
        active: &AlignmentPlan,
        frontier: PresentationFrontier,
    ) -> Result<PlanTransition, SyncError> {
        self.snapshot.reconcile_to(target, active, frontier)
    }
}

#[derive(Debug)]
struct FrozenGroup(SyncGroupSnapshot);

impl BeatMap for FrozenGroup {
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

impl SyncGroup for FrozenGroup {
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
        Beat::new(0.0).expect("invariant: zero beat is valid"),
    )
}

fn attach<G: SyncGroup>(
    group: &mut G,
    member: SyncMember<G::NestedGroup>,
) -> Result<SyncAdmission, SyncRejected<G::NestedGroup>> {
    let base = group
        .topology()
        .expect("invariant: the contract helper receives an observable group")
        .stamp();
    group.transact(SyncOperation::Topology {
        base,
        operations: vec![TopologyOperation::Attach { member }].into_boxed_slice(),
    })
}

fn status_and_acknowledge<G: SyncGroup>(
    group: &mut G,
    applied: SyncApplied,
) -> Result<(), SyncError> {
    let _: SyncStatusSnapshot = group.status();
    group.acknowledge(applied)
}

fn observe_status_variant(status: &SyncStatusSnapshot) {
    match status {
        SyncStatusSnapshot::Off { .. }
        | SyncStatusSnapshot::WaitingForMap { .. }
        | SyncStatusSnapshot::Prepared { .. }
        | SyncStatusSnapshot::Unavailable { .. }
        | SyncStatusSnapshot::Converging { .. }
        | SyncStatusSnapshot::Locked { .. } => {}
        _ => {}
    }
}

#[kithara::test]
fn attached_member_observes_later_asset_map_revision() {
    let group = host_map();
    let group_snapshot = group.snapshot();
    let (map, mut publisher) = AssetBeatMap::new(
        map_id(),
        NonZeroU32::new(48_000).expect("invariant: sample rate is non-zero"),
        48_001,
    );
    let initial = map.snapshot();
    let member: SyncMember<FrozenGroup> = SyncMember::Map {
        alignment: Some(BeatAlignment::new(zero(&initial), zero(&group_snapshot))),
        map: Box::new(map),
    };

    assert_eq!(
        member
            .snapshot_for(&group_snapshot)
            .expect("the initial live member topology is valid")
            .map()
            .stamp(),
        initial.stamp()
    );

    let published = publisher
        .publish(AssetMapUpdate::new(
            initial.stamp(),
            MapState::Building,
            Vec::new(),
        ))
        .expect("invariant: a live asset map may publish its next building revision");

    let current = member
        .snapshot_for(&group_snapshot)
        .expect("the updated live member topology is valid");
    assert_eq!(current.map().stamp(), published.stamp());
    assert_eq!(
        current
            .alignment()
            .expect("the established alignment remains materialized")
            .source()
            .stamp(),
        published.stamp()
    );

    let _attach_contract = attach::<FrozenGroup>;
    let _applied_contract = status_and_acknowledge::<FrozenGroup>;
    let _status_contract: fn(&SyncStatusSnapshot) = observe_status_variant;
}

fn align(parent: &BeatMapSnapshot, member: &BeatMapSnapshot) -> BeatAlignment {
    BeatAlignment::new(zero(member), zero(parent))
}

fn member_map() -> HostBeatMap {
    host_map()
}

fn promoted() -> SyncGroupSnapshot {
    let group_map = host_snapshot(map_id(), BeatMapRevision::first());
    SyncGroupSnapshot::try_new(group_map, TopologyRevision::first(), [])
        .expect("invariant: an empty fixture group is valid")
}

fn ordinary(map: &dyn BeatMap, alignment: BeatAlignment) -> SyncMemberSnapshot {
    SyncMemberSnapshot::new_map(map.snapshot(), Some(alignment))
}

fn nested(group: &SyncGroupSnapshot, alignment: BeatAlignment) -> SyncMemberSnapshot {
    SyncMemberSnapshot::new_group(group.clone(), Some(alignment))
}

#[kithara::test]
fn live_nested_group_preserves_its_concrete_owner_type() {
    let host = host_map();
    let parent = host.snapshot();
    let group = FrozenGroup(promoted());
    let group_id = group.id();
    let member = SyncMember::<FrozenGroup>::Group {
        alignment: Some(align(&parent, &group.snapshot())),
        group: Box::new(group),
    };

    assert_eq!(member.kind(), SyncMemberKind::Group);

    let snapshot = member
        .snapshot_for(&parent)
        .expect("the concrete nested group satisfies the live contract");

    assert_eq!(member.id(), group_id);
    assert_eq!(snapshot.map().id(), group_id);
    assert_eq!(
        snapshot
            .group_topology()
            .expect("the member remains a nested group")
            .stamp()
            .group_id(),
        group_id
    );
}

#[kithara::test]
fn group_promotes_map_geometry_without_self_membership() {
    let host = host_map();
    let topology = promoted();
    let map = topology.group_map();

    assert_ne!(map.id(), host.id());
    assert_eq!(map.axis(), host.snapshot().axis());
    assert_eq!(map.state(), host.snapshot().state());
    assert_eq!(topology.group_map().stamp(), map.stamp());
    assert_eq!(topology.stamp().group_id(), map.id());
    assert!(topology.members().is_empty());
}

#[kithara::test]
fn nested_groups_and_maps_share_one_object_safe_contract() {
    let child = promoted();
    let parent_map = host_snapshot(map_id(), BeatMapRevision::first());
    let leaf = member_map();
    let members = [
        ordinary(&leaf, align(&parent_map, &leaf.snapshot())),
        nested(&child, align(&parent_map, child.group_map())),
    ];
    let topology =
        SyncGroupSnapshot::try_new(parent_map.clone(), TopologyRevision::first(), members)
            .expect("invariant: distinct map and nested group form a tree");

    assert_eq!(topology.members().len(), 2);
    assert_eq!(topology.members()[0].map().id(), leaf.id());
    assert_eq!(
        topology.members()[1]
            .group_topology()
            .expect("invariant: second member is a group")
            .stamp(),
        child.stamp()
    );
}

#[kithara::test]
fn unavailable_members_join_without_a_fabricated_alignment() {
    let axis = MapAxis::Host(kithara_audio::HostAxis::new(
        NonZeroU32::new(48_000).expect("invariant: fixture sample rate is non-zero"),
        HostEpoch::new(0),
    ));
    let parent = BeatMapSnapshot::unavailable(map_id(), BeatMapRevision::first(), axis);
    let member = BeatMapSnapshot::unavailable(map_id(), BeatMapRevision::first(), axis);
    let live: SyncMember<FrozenGroup> = SyncMember::Map {
        alignment: None,
        map: Box::new(FrozenMap(member.clone())),
    };
    let member_snapshot = live
        .snapshot_for(&parent)
        .expect("an unavailable live member has a valid pending topology");

    let topology = SyncGroupSnapshot::try_new(parent, TopologyRevision::first(), [member_snapshot])
        .expect("an unavailable child can be owned before its alignment is known");

    assert_eq!(topology.members().len(), 1);
    assert_eq!(topology.members()[0].map().stamp(), member.stamp());
    assert_eq!(topology.members()[0].alignment(), None);
}

#[kithara::test]
fn live_member_rejects_a_snapshot_from_another_map_identity() {
    let parent = host_map().snapshot();
    let expected = map_id();
    let snapshot = host_map().snapshot();
    let given = snapshot.id();
    let member: SyncMember<FrozenGroup> = SyncMember::Map {
        alignment: None,
        map: Box::new(MismatchedMap {
            id: expected,
            snapshot,
        }),
    };

    let error = member
        .snapshot_for(&parent)
        .expect_err("one live map cannot publish another owner's snapshot");

    assert_eq!(error, SyncError::MapIdentityMismatch { expected, given });
}

#[kithara::test]
fn live_member_rejects_an_alignment_from_another_source_identity() {
    let parent = host_map().snapshot();
    let child = host_map().snapshot();
    let foreign = host_map().snapshot();
    let member: SyncMember<FrozenGroup> = SyncMember::Map {
        alignment: Some(BeatAlignment::new(zero(&foreign), zero(&parent))),
        map: Box::new(FrozenMap(child.clone())),
    };

    let error = member
        .snapshot_for(&parent)
        .expect_err("restamping must not adopt a foreign source identity");

    assert_eq!(
        error,
        SyncError::Topology(SyncGroupTopologyError::StaleSourceAlignment {
            expected: child.stamp(),
            given: foreign.stamp(),
        })
    );
}

#[kithara::test]
fn live_member_rejects_an_alignment_from_another_target_identity() {
    let parent = host_map().snapshot();
    let child = host_map().snapshot();
    let foreign = host_map().snapshot();
    let member: SyncMember<FrozenGroup> = SyncMember::Map {
        alignment: Some(BeatAlignment::new(zero(&child), zero(&foreign))),
        map: Box::new(FrozenMap(child.clone())),
    };

    let error = member
        .snapshot_for(&parent)
        .expect_err("restamping must not adopt a foreign target identity");

    assert_eq!(
        error,
        SyncError::Topology(SyncGroupTopologyError::StaleTargetAlignment {
            expected: parent.stamp(),
            given: foreign.stamp(),
        })
    );
}

#[kithara::test]
fn topology_rejects_self_membership() {
    let group_map = host_snapshot(map_id(), BeatMapRevision::first());
    let member = ordinary(&FrozenMap(group_map.clone()), align(&group_map, &group_map));

    let error = SyncGroupSnapshot::try_new(group_map, TopologyRevision::first(), [member])
        .expect_err("a group cannot contain its own map");

    assert!(matches!(error, SyncGroupTopologyError::SelfMember { .. }));
}

#[kithara::test]
fn topology_rejects_cycle_through_a_nested_group() {
    let parent_map = host_snapshot(map_id(), BeatMapRevision::first());
    let child_map = host_snapshot(map_id(), BeatMapRevision::first());
    let parent_leaf = ordinary(
        &FrozenMap(parent_map.clone()),
        align(&child_map, &parent_map),
    );
    let child =
        SyncGroupSnapshot::try_new(child_map.clone(), TopologyRevision::first(), [parent_leaf])
            .expect("invariant: child snapshot alone has no self-cycle");
    let child_member = nested(&child, align(&parent_map, &child_map));

    let error = SyncGroupSnapshot::try_new(parent_map, TopologyRevision::first(), [child_member])
        .expect_err("a nested path cannot return to its parent");

    assert!(matches!(error, SyncGroupTopologyError::Cycle { .. }));
}

#[kithara::test]
fn topology_rejects_a_nested_group_with_two_parent_edges() {
    let child = promoted();
    let parent_map = host_snapshot(map_id(), BeatMapRevision::first());
    let first = nested(&child, align(&parent_map, child.group_map()));
    let second = nested(&child, align(&parent_map, child.group_map()));

    let error = SyncGroupSnapshot::try_new(parent_map, TopologyRevision::first(), [first, second])
        .expect_err("one group cannot have two parent edges in the same tree");

    assert!(matches!(
        error,
        SyncGroupTopologyError::MultipleParents { .. }
    ));
}

#[kithara::test]
fn topology_rejects_one_leaf_repeated_across_nested_groups() {
    let parent_map = host_snapshot(map_id(), BeatMapRevision::first());
    let leaf = member_map();
    let children = [0, 1].map(|_| {
        let child_map = host_snapshot(map_id(), BeatMapRevision::first());
        let leaf_member = ordinary(&leaf, align(&child_map, &leaf.snapshot()));
        SyncGroupSnapshot::try_new(child_map.clone(), TopologyRevision::first(), [leaf_member])
            .map(|group| nested(&group, align(&parent_map, &child_map)))
            .expect("invariant: each child alone has one distinct leaf path")
    });

    let error = SyncGroupSnapshot::try_new(parent_map, TopologyRevision::first(), children)
        .expect_err("a leaf cannot appear twice in one tree");

    assert!(matches!(
        error,
        SyncGroupTopologyError::DuplicateLeaf { .. }
    ));
}

#[kithara::test]
fn topology_rejects_a_stale_parent_alignment() {
    let group_map = host_snapshot(map_id(), BeatMapRevision::first());
    let stale_group_map = host_snapshot(
        group_map.id(),
        group_map
            .revision()
            .checked_next()
            .expect("invariant: fixture map revision can advance"),
    );
    let leaf = member_map();
    let member = ordinary(
        &leaf,
        BeatAlignment::new(zero(&leaf.snapshot()), zero(&stale_group_map)),
    );

    let error = SyncGroupSnapshot::try_new(group_map, TopologyRevision::first(), [member])
        .expect_err("an alignment from another parent revision must be stale");

    assert!(matches!(
        error,
        SyncGroupTopologyError::StaleTargetAlignment { .. }
    ));
}

#[kithara::test]
fn topology_rejects_a_stale_member_alignment() {
    let group_map = host_snapshot(map_id(), BeatMapRevision::first());
    let leaf = member_map();
    let leaf_snapshot = leaf.snapshot();
    let stale_leaf = host_snapshot(
        leaf_snapshot.id(),
        leaf_snapshot
            .revision()
            .checked_next()
            .expect("invariant: fixture map revision can advance"),
    );
    let member = ordinary(
        &FrozenMap(leaf_snapshot),
        BeatAlignment::new(zero(&stale_leaf), zero(&group_map)),
    );

    let error = SyncGroupSnapshot::try_new(group_map, TopologyRevision::first(), [member])
        .expect_err("an alignment from another member revision must be stale");

    assert!(matches!(
        error,
        SyncGroupTopologyError::StaleSourceAlignment { .. }
    ));
}
