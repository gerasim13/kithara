use std::fmt;

use super::{BeatAlignment, SyncGroup, SyncMemberKind};
use crate::{BeatMap, BeatMapId};

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
}

impl<G: SyncGroup> From<&SyncMember<G>> for BeatMapId {
    fn from(member: &SyncMember<G>) -> Self {
        match member {
            SyncMember::Map { map, .. } => map.id(),
            SyncMember::Group { group, .. } => group.id(),
        }
    }
}
