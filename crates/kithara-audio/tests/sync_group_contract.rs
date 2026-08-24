use std::num::NonZeroU32;

use kithara_audio::{
    Beat, BeatMap, BeatMapId, BeatMapRevision, BeatMapSnapshot, HostBeatMap, HostEpoch, MapPoint,
    MapStamp, SessionAnchor, SessionBeat, SessionFrame, SyncAlignment, SyncGroup, SyncGroupEdit,
    SyncGroupMember, SyncGroupSnapshot, SyncGroupTopologyError, SyncGroupUpdate, TopologyRevision,
    TopologyStamp,
};
use kithara_test_utils::kithara;

fn map_id() -> BeatMapId {
    BeatMapId::allocate().expect("invariant: test map identity space is available")
}

fn host_map() -> HostBeatMap {
    let sample_rate = NonZeroU32::new(48_000).expect("invariant: sample rate is non-zero");
    let anchor = SessionAnchor::new(
        SessionFrame::new(0),
        SessionBeat::new(0.0).expect("invariant: host beat is finite"),
        2.0,
        sample_rate,
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
struct FrozenMap(BeatMapSnapshot);

impl BeatMap for FrozenMap {
    fn id(&self) -> BeatMapId {
        self.0.id()
    }

    fn snapshot(&self) -> BeatMapSnapshot {
        self.0.clone()
    }
}

fn read_group(group: &dyn SyncGroup) -> (BeatMapSnapshot, SyncGroupSnapshot) {
    (group.snapshot(), group.topology())
}

fn zero(map: &BeatMapSnapshot) -> MapPoint<Beat> {
    MapPoint::new(
        map.stamp(),
        Beat::new(0.0).expect("invariant: zero beat is valid"),
    )
}

fn align(group: &BeatMapSnapshot, member: &BeatMapSnapshot) -> SyncAlignment {
    SyncAlignment::new(zero(group), zero(member))
}

fn member_map() -> HostBeatMap {
    host_map()
}

fn promoted(map: &dyn BeatMap) -> SyncGroupSnapshot {
    SyncGroupSnapshot::try_from(map).expect("invariant: test group identity space is available")
}

fn ordinary(map: &dyn BeatMap, alignment: SyncAlignment) -> SyncGroupMember {
    SyncGroupMember::from((map, alignment))
}

fn nested(group: &dyn SyncGroup, alignment: SyncAlignment) -> SyncGroupMember {
    SyncGroupMember::from((group, alignment))
}

#[kithara::test]
fn group_promotes_map_geometry_without_self_membership() {
    let host = host_map();
    let group = promoted(&host);
    let (map, topology) = read_group(&group);

    assert_ne!(map.id(), host.id());
    assert_eq!(map.axis(), host.snapshot().axis());
    assert_eq!(map.state(), host.snapshot().state());
    assert_eq!(topology.group_map().stamp(), map.stamp());
    assert_eq!(topology.stamp().group_id(), map.id());
    assert!(topology.members().is_empty());
}

#[kithara::test]
fn nested_groups_and_maps_share_one_object_safe_contract() {
    let host = host_map();
    let child = promoted(&host);
    let parent_map = host
        .snapshot()
        .restamp(MapStamp::new(map_id(), BeatMapRevision::first()));
    let leaf = member_map();
    let members = [
        ordinary(&leaf, align(&parent_map, &leaf.snapshot())),
        nested(&child, align(&parent_map, &child.snapshot())),
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
        child.topology().stamp()
    );
}

#[kithara::test]
fn topology_rejects_self_membership() {
    let host = host_map();
    let group_map = host
        .snapshot()
        .restamp(MapStamp::new(map_id(), BeatMapRevision::first()));
    let member = ordinary(&FrozenMap(group_map.clone()), align(&group_map, &group_map));

    let error = SyncGroupSnapshot::try_new(group_map, TopologyRevision::first(), [member])
        .expect_err("a group cannot contain its own map");

    assert!(matches!(error, SyncGroupTopologyError::SelfMember { .. }));
}

#[kithara::test]
fn topology_rejects_cycle_through_a_nested_group() {
    let host = host_map();
    let parent_map = host
        .snapshot()
        .restamp(MapStamp::new(map_id(), BeatMapRevision::first()));
    let child_map = host
        .snapshot()
        .restamp(MapStamp::new(map_id(), BeatMapRevision::first()));
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
    let host = host_map();
    let child = promoted(&host);
    let parent_map = host
        .snapshot()
        .restamp(MapStamp::new(map_id(), BeatMapRevision::first()));
    let first = nested(&child, align(&parent_map, &child.snapshot()));
    let second = nested(&child, align(&parent_map, &child.snapshot()));

    let error = SyncGroupSnapshot::try_new(parent_map, TopologyRevision::first(), [first, second])
        .expect_err("one group cannot have two parent edges in the same tree");

    assert!(matches!(
        error,
        SyncGroupTopologyError::MultipleParents { .. }
    ));
}

#[kithara::test]
fn topology_rejects_one_leaf_repeated_across_nested_groups() {
    let host = host_map();
    let parent_map = host
        .snapshot()
        .restamp(MapStamp::new(map_id(), BeatMapRevision::first()));
    let leaf = member_map();
    let children = [0, 1].map(|_| {
        let child_map = host
            .snapshot()
            .restamp(MapStamp::new(map_id(), BeatMapRevision::first()));
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
fn add_remove_and_replace_prepare_one_atomic_topology_candidate() {
    let host = host_map();
    let group = promoted(&host);
    let original = group.topology();
    let first = member_map();
    let second = member_map();
    let replacement = member_map();
    let edits = [
        SyncGroupEdit::Add(ordinary(
            &first,
            align(original.group_map(), &first.snapshot()),
        )),
        SyncGroupEdit::Add(ordinary(
            &second,
            align(original.group_map(), &second.snapshot()),
        )),
        SyncGroupEdit::Remove(first.id()),
        SyncGroupEdit::Replace {
            member_id: second.id(),
            replacement: ordinary(
                &replacement,
                align(original.group_map(), &replacement.snapshot()),
            ),
        },
    ];
    let update = SyncGroupUpdate::new(original.stamp(), edits);

    let group_contract: &dyn SyncGroup = &group;
    let candidate = group_contract
        .prepare_update(&update)
        .expect("invariant: the whole edit sequence is valid");

    assert_eq!(group.topology(), original);
    assert_eq!(candidate.group_map().stamp(), original.group_map().stamp());
    assert_eq!(
        candidate.stamp().revision(),
        original
            .stamp()
            .revision()
            .checked_next()
            .expect("invariant: fixture topology revision can advance")
    );
    assert_eq!(candidate.members().len(), 1);
    assert_eq!(candidate.members()[0].map().id(), replacement.id());
}

#[kithara::test]
fn topology_rejects_a_stale_parent_alignment() {
    let host = host_map();
    let group_map = host
        .snapshot()
        .restamp(MapStamp::new(map_id(), BeatMapRevision::first()));
    let stale_group_map = group_map.restamp(MapStamp::new(
        group_map.id(),
        group_map
            .revision()
            .checked_next()
            .expect("invariant: fixture map revision can advance"),
    ));
    let leaf = member_map();
    let member = ordinary(
        &leaf,
        SyncAlignment::new(zero(&stale_group_map), zero(&leaf.snapshot())),
    );

    let error = SyncGroupSnapshot::try_new(group_map, TopologyRevision::first(), [member])
        .expect_err("an alignment from another parent revision must be stale");

    assert!(matches!(
        error,
        SyncGroupTopologyError::StaleGroupAlignment { .. }
    ));
}

#[kithara::test]
fn topology_rejects_a_stale_member_alignment() {
    let host = host_map();
    let group_map = host
        .snapshot()
        .restamp(MapStamp::new(map_id(), BeatMapRevision::first()));
    let leaf = member_map();
    let leaf_snapshot = leaf.snapshot();
    let stale_leaf = leaf_snapshot.restamp(MapStamp::new(
        leaf_snapshot.id(),
        leaf_snapshot
            .revision()
            .checked_next()
            .expect("invariant: fixture map revision can advance"),
    ));
    let member = ordinary(
        &FrozenMap(leaf_snapshot),
        SyncAlignment::new(zero(&group_map), zero(&stale_leaf)),
    );

    let error = SyncGroupSnapshot::try_new(group_map, TopologyRevision::first(), [member])
        .expect_err("an alignment from another member revision must be stale");

    assert!(matches!(
        error,
        SyncGroupTopologyError::StaleMemberAlignment { .. }
    ));
}

#[kithara::test]
fn stale_update_base_leaves_the_published_snapshot_unchanged() {
    let host = host_map();
    let group = promoted(&host);
    let original = group.topology();
    let stale = TopologyStamp::new(
        original.stamp().group_id(),
        original
            .stamp()
            .revision()
            .checked_next()
            .expect("invariant: fixture topology revision can advance"),
    );
    let update = SyncGroupUpdate::new(stale, []);

    let error = group
        .prepare_update(&update)
        .expect_err("an update from another topology revision must be stale");

    assert!(matches!(
        error,
        SyncGroupTopologyError::StaleTopology { expected, given }
            if expected == original.stamp() && given == stale
    ));
    assert_eq!(group.topology(), original);
}

#[kithara::test]
fn rejected_update_leaves_the_published_snapshot_unchanged() {
    let host = host_map();
    let group = promoted(&host);
    let original = group.topology();
    let missing = map_id();
    let update = SyncGroupUpdate::new(original.stamp(), [SyncGroupEdit::Remove(missing)]);

    let error = group
        .prepare_update(&update)
        .expect_err("removing an unknown member must reject the whole candidate");

    assert!(matches!(
        error,
        SyncGroupTopologyError::MemberNotFound { member_id } if member_id == missing
    ));
    assert_eq!(group.topology(), original);
}
