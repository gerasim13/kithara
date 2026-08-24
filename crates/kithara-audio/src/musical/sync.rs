use std::{collections::BTreeSet, num::NonZeroU64};

use kithara_platform::sync::Arc;

use super::{
    Beat, BeatMap, BeatMapId, BeatMapIdAllocationError, BeatMapRevision, BeatMapSnapshot, MapPoint,
    MapStamp,
};

/// Monotonic revision of one synchronization-group topology.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    derive_more::Display,
    derive_more::Into,
)]
#[display("{_0}")]
#[into(u64)]
#[repr(transparent)]
pub struct TopologyRevision(NonZeroU64);

impl TopologyRevision {
    /// Returns the first revision assigned by a group owner.
    #[must_use]
    pub const fn first() -> Self {
        Self(NonZeroU64::MIN)
    }

    /// Returns the next owner-assigned revision, or `None` on exhaustion.
    #[must_use]
    pub fn checked_next(self) -> Option<Self> {
        self.0
            .get()
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .map(Self)
    }
}

/// Identity and immutable revision of one group topology snapshot.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub struct TopologyStamp {
    group_id: BeatMapId,
    revision: TopologyRevision,
}

impl TopologyStamp {
    /// Creates a composite topology stamp.
    #[must_use]
    pub const fn new(group_id: BeatMapId, revision: TopologyRevision) -> Self {
        Self { group_id, revision }
    }

    /// Returns the stable identity of the group map.
    #[must_use]
    pub const fn group_id(self) -> BeatMapId {
        self.group_id
    }

    /// Returns the immutable topology revision.
    #[must_use]
    pub const fn revision(self) -> TopologyRevision {
        self.revision
    }
}

/// A beat on a group map aligned with a beat on one direct member map.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct SyncAlignment {
    group: MapPoint<Beat>,
    member: MapPoint<Beat>,
}

impl SyncAlignment {
    /// Creates one immutable alignment edge.
    #[must_use]
    pub const fn new(group: MapPoint<Beat>, member: MapPoint<Beat>) -> Self {
        Self { group, member }
    }

    /// Returns the point on the direct parent group.
    #[must_use]
    pub const fn group(&self) -> MapPoint<Beat> {
        self.group
    }

    /// Returns the corresponding point on the member map.
    #[must_use]
    pub const fn member(&self) -> MapPoint<Beat> {
        self.member
    }
}

/// One direct map or nested-group edge in an immutable topology.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SyncGroupMember {
    alignment: SyncAlignment,
    map: BeatMapSnapshot,
    group: Option<SyncGroupSnapshot>,
}

impl SyncGroupMember {
    /// Returns the direct member's frozen map.
    #[must_use]
    pub const fn map(&self) -> &BeatMapSnapshot {
        &self.map
    }

    /// Returns the alignment edge from the direct parent.
    #[must_use]
    pub const fn alignment(&self) -> SyncAlignment {
        self.alignment
    }

    /// Returns the frozen nested topology when this member is a group.
    #[must_use]
    pub const fn group_topology(&self) -> Option<&SyncGroupSnapshot> {
        self.group.as_ref()
    }
}

/// Freezes one ordinary map behind the shared readable protocol.
impl From<(&dyn BeatMap, SyncAlignment)> for SyncGroupMember {
    fn from((map, alignment): (&dyn BeatMap, SyncAlignment)) -> Self {
        Self {
            alignment,
            map: map.snapshot(),
            group: None,
        }
    }
}

/// Freezes one nested group behind the shared recursive protocol.
impl From<(&dyn SyncGroup, SyncAlignment)> for SyncGroupMember {
    fn from((group, alignment): (&dyn SyncGroup, SyncAlignment)) -> Self {
        let group = group.topology();
        Self {
            alignment,
            map: group.group_map.clone(),
            group: Some(group),
        }
    }
}

/// One immutable observation of a synchronization group's map and members.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SyncGroupSnapshot {
    group_map: BeatMapSnapshot,
    stamp: TopologyStamp,
    members: Arc<[SyncGroupMember]>,
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
        I: IntoIterator<Item = SyncGroupMember>,
    {
        let members: Arc<[SyncGroupMember]> = members.into_iter().collect();
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
    pub fn members(&self) -> &[SyncGroupMember] {
        &self.members
    }

    fn prepare_update(&self, update: &SyncGroupUpdate) -> Result<Self, SyncGroupTopologyError> {
        if update.base != self.stamp {
            return Err(SyncGroupTopologyError::StaleTopology {
                expected: self.stamp,
                given: update.base,
            });
        }
        let revision = self
            .stamp
            .revision()
            .checked_next()
            .ok_or(SyncGroupTopologyError::RevisionExhausted)?;
        let mut members = self.members.to_vec();
        for edit in update.edits.iter() {
            match edit {
                SyncGroupEdit::Add(member) => members.push(member.clone()),
                SyncGroupEdit::Remove(member_id) => {
                    let index = direct_member_index(&members, *member_id)?;
                    members.remove(index);
                }
                SyncGroupEdit::Replace {
                    member_id,
                    replacement,
                } => {
                    let index = direct_member_index(&members, *member_id)?;
                    members[index] = replacement.clone();
                }
            }
        }
        Self::try_new(self.group_map.clone(), revision, members)
    }
}

/// Promotes any readable map into an independent empty group.
///
/// The source geometry is copied into a fresh map identity. The source map is
/// not retained as a leader, member, or alignment edge.
impl TryFrom<&dyn BeatMap> for SyncGroupSnapshot {
    type Error = BeatMapIdAllocationError;

    fn try_from(map: &dyn BeatMap) -> Result<Self, Self::Error> {
        let group_map = map.snapshot().restamp(MapStamp::new(
            BeatMapId::allocate()?,
            BeatMapRevision::first(),
        ));
        Ok(Self {
            stamp: TopologyStamp::new(group_map.id(), TopologyRevision::first()),
            group_map,
            members: Arc::from([]),
        })
    }
}

impl BeatMap for SyncGroupSnapshot {
    delegate::delegate! {
        to self.group_map {
            fn id(&self) -> BeatMapId;
            #[call(clone)]
            fn snapshot(&self) -> BeatMapSnapshot;
        }
    }
}

/// One topology edit prepared against an immutable base stamp.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum SyncGroupEdit {
    /// Adds one direct map or nested group.
    Add(SyncGroupMember),
    /// Removes one direct member by map identity.
    Remove(BeatMapId),
    /// Replaces one direct member while preserving edit ordering.
    Replace {
        /// Identity of the direct member being replaced.
        member_id: BeatMapId,
        /// New direct member edge.
        replacement: SyncGroupMember,
    },
}

/// Atomic candidate edit sequence for one exact topology revision.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SyncGroupUpdate {
    base: TopologyStamp,
    edits: Arc<[SyncGroupEdit]>,
}

impl SyncGroupUpdate {
    /// Freezes an ordered edit sequence against `base`.
    #[must_use]
    pub fn new<I>(base: TopologyStamp, edits: I) -> Self
    where
        I: IntoIterator<Item = SyncGroupEdit>,
    {
        Self {
            base,
            edits: edits.into_iter().collect(),
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
    /// The alignment's parent point belongs to another map revision.
    #[error("alignment parent stamp is {given:?}, expected {expected:?}")]
    StaleGroupAlignment { expected: MapStamp, given: MapStamp },
    /// The alignment's member point belongs to another map revision.
    #[error("alignment member stamp is {given:?}, expected {expected:?}")]
    StaleMemberAlignment { expected: MapStamp, given: MapStamp },
    /// An edit addressed a member absent from the direct topology.
    #[error("direct member {member_id} does not exist")]
    MemberNotFound { member_id: BeatMapId },
    /// An update was prepared against another published topology revision.
    #[error("topology update base is {given:?}, expected {expected:?}")]
    StaleTopology {
        expected: TopologyStamp,
        given: TopologyStamp,
    },
    /// The topology revision identity space is exhausted.
    #[error("topology revision space is exhausted")]
    RevisionExhausted,
}

/// Read-only protocol for a recursive group of musical maps.
///
/// The topology's group-map stamp must equal `snapshot().stamp()`, and its
/// group identity must equal `id()`.
pub trait SyncGroup: BeatMap {
    /// Returns one immutable topology snapshot for a complete calculation.
    fn topology(&self) -> SyncGroupSnapshot;

    /// Validates an atomic edit sequence without mutating the published group.
    ///
    /// # Errors
    ///
    /// Returns [`SyncGroupTopologyError`] when the base stamp is stale or the
    /// complete candidate violates topology invariants.
    fn prepare_update(
        &self,
        update: &SyncGroupUpdate,
    ) -> Result<SyncGroupSnapshot, SyncGroupTopologyError> {
        self.topology().prepare_update(update)
    }
}

impl SyncGroup for SyncGroupSnapshot {
    fn topology(&self) -> SyncGroupSnapshot {
        self.clone()
    }
}

fn direct_member_index(
    members: &[SyncGroupMember],
    member_id: BeatMapId,
) -> Result<usize, SyncGroupTopologyError> {
    members
        .iter()
        .position(|member| member.map.id() == member_id)
        .ok_or(SyncGroupTopologyError::MemberNotFound { member_id })
}

fn validate_edges(
    group_map: &BeatMapSnapshot,
    members: &[SyncGroupMember],
) -> Result<(), SyncGroupTopologyError> {
    for member in members {
        let group_stamp = member.alignment.group().stamp();
        if group_stamp != group_map.stamp() {
            return Err(SyncGroupTopologyError::StaleGroupAlignment {
                expected: group_map.stamp(),
                given: group_stamp,
            });
        }
        let member_stamp = member.alignment.member().stamp();
        if member_stamp != member.map.stamp() {
            return Err(SyncGroupTopologyError::StaleMemberAlignment {
                expected: member.map.stamp(),
                given: member_stamp,
            });
        }
    }
    Ok(())
}

fn validate_tree(
    root: BeatMapId,
    members: &[SyncGroupMember],
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
