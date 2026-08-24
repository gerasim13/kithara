use std::fmt;

use super::{
    BeatAlignment, SyncError, SyncGroup, SyncGroupTopologyError, SyncMemberKind, SyncMemberSnapshot,
};
use crate::musical::{BeatMap, BeatMapId, BeatMapSnapshot, MapPoint, MapStamp};

/// One exclusively owned live map or statically typed nested synchronization group.
pub enum SyncMember<G: SyncGroup> {
    /// A readable live map.
    Map {
        /// Alignment from this member to its direct parent, once both maps
        /// expose usable geometry.
        alignment: Option<BeatAlignment>,
        /// Live map handle owned by the parent group.
        map: Box<dyn BeatMap>,
    },
    /// A live nested synchronization group.
    Group {
        /// Alignment from this member to its direct parent, once both maps
        /// expose usable geometry.
        alignment: Option<BeatAlignment>,
        /// Live group owned by the parent group.
        group: Box<G>,
    },
}

impl<G: SyncGroup> fmt::Debug for SyncMember<G> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Map { alignment, map } => formatter
                .debug_struct("SyncMember::Map")
                .field("alignment", alignment)
                .field("map_id", &map.id())
                .finish(),
            Self::Group { alignment, group } => formatter
                .debug_struct("SyncMember::Group")
                .field("alignment", alignment)
                .field("group_id", &group.id())
                .finish(),
        }
    }
}

impl<G: SyncGroup> SyncMember<G> {
    /// Returns the stable identity of this live member.
    #[must_use]
    pub fn id(&self) -> BeatMapId {
        self.into()
    }

    /// Returns the direct alignment from this member to its parent.
    #[must_use]
    pub const fn alignment(&self) -> Option<BeatAlignment> {
        match self {
            Self::Map { alignment, .. } | Self::Group { alignment, .. } => *alignment,
        }
    }

    /// Returns whether this member is an ordinary map or a nested group.
    #[must_use]
    pub const fn kind(&self) -> SyncMemberKind {
        match self {
            Self::Map { .. } => SyncMemberKind::Map,
            Self::Group { .. } => SyncMemberKind::Group,
        }
    }

    /// Materializes the current member and restamps an established alignment
    /// to the current child and parent revisions.
    ///
    /// # Errors
    ///
    /// Returns [`SyncError`] for an identity or nested-topology violation.
    pub fn snapshot_for(&self, parent: &BeatMapSnapshot) -> Result<SyncMemberSnapshot, SyncError> {
        match self {
            Self::Map { alignment, map } => {
                let expected = map.id();
                let map = map.snapshot();
                if map.id() != expected {
                    return Err(SyncError::MapIdentityMismatch {
                        expected,
                        given: map.id(),
                    });
                }
                let alignment = restamp_alignment(*alignment, map.stamp(), parent.stamp())?;
                Ok(SyncMemberSnapshot::new_map(map, alignment))
            }
            Self::Group { alignment, group } => {
                let expected = group.id();
                let group = group.topology()?;
                if group.stamp().group_id() != expected {
                    return Err(SyncError::MapIdentityMismatch {
                        expected,
                        given: group.stamp().group_id(),
                    });
                }
                let alignment =
                    restamp_alignment(*alignment, group.group_map().stamp(), parent.stamp())?;
                Ok(SyncMemberSnapshot::new_group(group, alignment))
            }
        }
    }
}

impl<G: SyncGroup> From<&SyncMember<G>> for BeatMapId {
    fn from(member: &SyncMember<G>) -> Self {
        match member {
            SyncMember::Map { map, .. } => map.id(),
            SyncMember::Group { group, .. } => group.id(),
        }
    }
}

fn restamp_alignment(
    alignment: Option<BeatAlignment>,
    source: MapStamp,
    target: MapStamp,
) -> Result<Option<BeatAlignment>, SyncError> {
    alignment
        .map(|alignment| {
            let given_source = alignment.source().stamp();
            if given_source.map_id() != source.map_id() {
                return Err(SyncGroupTopologyError::StaleSourceAlignment {
                    expected: source,
                    given: given_source,
                }
                .into());
            }
            let given_target = alignment.target().stamp();
            if given_target.map_id() != target.map_id() {
                return Err(SyncGroupTopologyError::StaleTargetAlignment {
                    expected: target,
                    given: given_target,
                }
                .into());
            }
            Ok(BeatAlignment::new(
                MapPoint::new(source, *alignment.source().value()),
                MapPoint::new(target, *alignment.target().value()),
            ))
        })
        .transpose()
}
