use std::collections::BTreeSet;

use kithara_platform::sync::Arc;

use super::{
    AlignmentPlan, AlignmentRequest, BeatAlignment, PlanTransition, PresentationFrontier,
    SyncError, TopologyRevision, TopologyStamp,
};
use crate::musical::{BeatMap, BeatMapId, BeatMapSnapshot, MapStamp};

/// One immutable observation of a direct live member.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SyncMemberSnapshot {
    alignment: Option<BeatAlignment>,
    map: BeatMapSnapshot,
    group: Option<SyncGroupSnapshot>,
}

impl SyncMemberSnapshot {
    /// Freezes one ordinary map edge, including a pending edge without alignment.
    #[must_use]
    pub const fn new_map(map: BeatMapSnapshot, alignment: Option<BeatAlignment>) -> Self {
        Self {
            alignment,
            map,
            group: None,
        }
    }

    /// Freezes one nested group edge, including a pending edge without alignment.
    #[must_use]
    pub fn new_group(group: SyncGroupSnapshot, alignment: Option<BeatAlignment>) -> Self {
        Self {
            alignment,
            map: group.group_map.clone(),
            group: Some(group),
        }
    }

    /// Returns the direct member's frozen map.
    #[must_use]
    pub const fn map(&self) -> &BeatMapSnapshot {
        &self.map
    }

    /// Returns the alignment edge from the direct parent.
    #[must_use]
    pub const fn alignment(&self) -> Option<BeatAlignment> {
        self.alignment
    }

    /// Returns the frozen nested topology when this member is a group.
    #[must_use]
    pub const fn group_topology(&self) -> Option<&SyncGroupSnapshot> {
        self.group.as_ref()
    }
}

/// One immutable observation of a synchronization group's map and members.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SyncGroupSnapshot {
    group_map: BeatMapSnapshot,
    stamp: TopologyStamp,
    members: Arc<[SyncMemberSnapshot]>,
}

impl SyncGroupSnapshot {
    /// Creates and validates a topology tree.
    ///
    /// # Errors
    ///
    /// Returns [`SyncGroupTopologyError`] when an edge uses stale map stamps,
    /// repeats a member, or makes the tree recursive.
    pub fn try_new<I>(
        group_map: BeatMapSnapshot,
        revision: TopologyRevision,
        members: I,
    ) -> Result<Self, SyncGroupTopologyError>
    where
        I: IntoIterator<Item = SyncMemberSnapshot>,
    {
        let members: Arc<[SyncMemberSnapshot]> = members.into_iter().collect();
        validate_edges(&group_map, &members)?;
        validate_tree(group_map.id(), &members)?;
        Ok(Self {
            stamp: TopologyStamp::new(group_map.id(), revision),
            group_map,
            members,
        })
    }

    /// Returns the group's authoritative musical-coordinate snapshot.
    #[must_use]
    pub const fn group_map(&self) -> &BeatMapSnapshot {
        &self.group_map
    }

    /// Returns the topology identity and revision.
    #[must_use]
    pub const fn stamp(&self) -> TopologyStamp {
        self.stamp
    }

    /// Returns the direct members frozen into this topology revision.
    #[must_use]
    pub fn members(&self) -> &[SyncMemberSnapshot] {
        &self.members
    }
}

impl BeatMap for SyncGroupSnapshot {
    delegate::delegate! {
        to self.group_map {
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

/// A synchronization topology violates tree, identity, or revision rules.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum SyncGroupTopologyError {
    /// A direct ordinary member is the group map itself.
    #[error("group map {group_id} cannot be its own member")]
    SelfMember { group_id: BeatMapId },
    /// A nested path returns to an ancestor group.
    #[error("nested group path returns to ancestor map {group_id}")]
    Cycle { group_id: BeatMapId },
    /// One nested group appears below more than one parent edge.
    #[error("nested group {group_id} has multiple parent edges")]
    MultipleParents { group_id: BeatMapId },
    /// One ordinary map appears more than once in the same topology tree.
    #[error("leaf map {member_id} appears more than once")]
    DuplicateLeaf { member_id: BeatMapId },
    /// One map identity appears as both a nested group and an ordinary leaf.
    #[error("map {member_id} appears as both a group and a leaf")]
    ConflictingMemberKind { member_id: BeatMapId },
    /// The alignment's target point belongs to another map revision.
    #[error("alignment target stamp is {given:?}, expected {expected:?}")]
    StaleTargetAlignment { expected: MapStamp, given: MapStamp },
    /// The alignment's source point belongs to another map revision.
    #[error("alignment source stamp is {given:?}, expected {expected:?}")]
    StaleSourceAlignment { expected: MapStamp, given: MapStamp },
}

fn validate_edges(
    group_map: &BeatMapSnapshot,
    members: &[SyncMemberSnapshot],
) -> Result<(), SyncGroupTopologyError> {
    for member in members {
        if let Some(alignment) = member.alignment {
            let target_stamp = alignment.target().stamp();
            if target_stamp != group_map.stamp() {
                return Err(SyncGroupTopologyError::StaleTargetAlignment {
                    expected: group_map.stamp(),
                    given: target_stamp,
                });
            }
            let source_stamp = alignment.source().stamp();
            if source_stamp != member.map.stamp() {
                return Err(SyncGroupTopologyError::StaleSourceAlignment {
                    expected: member.map.stamp(),
                    given: source_stamp,
                });
            }
        }
    }
    Ok(())
}

fn validate_tree(
    root: BeatMapId,
    members: &[SyncMemberSnapshot],
) -> Result<(), SyncGroupTopologyError> {
    let mut groups = BTreeSet::from([root]);
    let mut leaves = BTreeSet::new();
    for member in members {
        match member.group_topology() {
            Some(group) => visit_group(group, root, &mut groups, &mut leaves)?,
            None if member.map.id() == root => {
                return Err(SyncGroupTopologyError::SelfMember { group_id: root });
            }
            None => visit_leaf(member.map.id(), &groups, &mut leaves)?,
        }
    }
    Ok(())
}

fn visit_group(
    group: &SyncGroupSnapshot,
    root: BeatMapId,
    groups: &mut BTreeSet<BeatMapId>,
    leaves: &mut BTreeSet<BeatMapId>,
) -> Result<(), SyncGroupTopologyError> {
    let group_id = group.group_map.id();
    if group_id == root {
        return Err(SyncGroupTopologyError::Cycle { group_id });
    }
    if leaves.contains(&group_id) {
        return Err(SyncGroupTopologyError::ConflictingMemberKind {
            member_id: group_id,
        });
    }
    if !groups.insert(group_id) {
        return Err(SyncGroupTopologyError::MultipleParents { group_id });
    }
    for member in group.members() {
        match member.group_topology() {
            Some(child) => visit_group(child, root, groups, leaves)?,
            None if member.map.id() == root => {
                return Err(SyncGroupTopologyError::Cycle { group_id: root });
            }
            None => visit_leaf(member.map.id(), groups, leaves)?,
        }
    }
    Ok(())
}

fn visit_leaf(
    member_id: BeatMapId,
    groups: &BTreeSet<BeatMapId>,
    leaves: &mut BTreeSet<BeatMapId>,
) -> Result<(), SyncGroupTopologyError> {
    if groups.contains(&member_id) {
        return Err(SyncGroupTopologyError::ConflictingMemberKind { member_id });
    }
    if !leaves.insert(member_id) {
        return Err(SyncGroupTopologyError::DuplicateLeaf { member_id });
    }
    Ok(())
}
