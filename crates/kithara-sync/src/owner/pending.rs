use std::ops::Range;

use kithara_signal::{SessionFrame, TransportRevision};
use kithara_warp::{BeatGridId, MapRegion};

use crate::{
    AlignmentSource, LoadGeneration, SyncEffect, SyncExecutionReject, SyncOperationId,
    SyncPreparation, SyncTransition,
};

/// The one unapplied decision a group holds for a direct member.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Pending {
    /// The member's preparation is issued to its executor.
    Prepared {
        preparation: SyncPreparation,
        entry: Entry,
        phase: Phase,
    },
    /// The member's preparation needs grid coverage not yet published.
    Waiting {
        member: BeatGridId,
        operation: SyncOperationId,
        load: LoadGeneration,
        transport: TransportRevision,
        required: MapRegion,
    },
    /// The member's decision waits for its Host to observe the member's
    /// source afresh and plan it once more: it missed its first boundary,
    /// for the reason `missed` gives, or it sounded on a member grid refined
    /// since.
    Replanning {
        member: BeatGridId,
        operation: SyncOperationId,
        load: LoadGeneration,
        transport: TransportRevision,
        missed: Option<SyncExecutionReject>,
    },
}

/// How a preparation moves its member.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Entry {
    /// A silent member starts to sound, and its activation must stay inside
    /// the launch window it was asked for.
    Launch(Range<SessionFrame>),
    /// A sounding member leaves its applied map.
    Replace,
    /// A public deck entry replans from its actual source inside this window.
    Public {
        source: AlignmentSource,
        window: Range<SessionFrame>,
    },
    /// A sounding member moves to an exact cue while its applied map keeps
    /// sounding; a new group grid withdraws it rather than carrying it.
    Relocate,
}

/// How far the executor carried a preparation out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Phase {
    /// Issued and not acknowledged yet.
    Issued,
    /// Held by the executor, which may still drop it.
    Installed,
    /// Committed to the output; only its presentation or a new session axis
    /// ends it.
    Armed,
}

impl Entry {
    /// Whether a transient miss is planned once more: a public entry or a
    /// sounding retarget follows the group's current intent, while a launch
    /// window or a relocation cue ages with the output.
    pub(super) const fn replans_a_miss(&self) -> bool {
        matches!(self, Self::Public { .. } | Self::Replace)
    }
}

impl Pending {
    pub(super) fn member(&self) -> BeatGridId {
        match self {
            Self::Prepared { preparation, .. } => preparation.stamp().member().grid_id(),
            Self::Waiting { member, .. } | Self::Replanning { member, .. } => *member,
        }
    }

    pub(super) fn operation(&self) -> SyncOperationId {
        match self {
            Self::Prepared { preparation, .. } => preparation.stamp().operation(),
            Self::Waiting { operation, .. } | Self::Replanning { operation, .. } => *operation,
        }
    }

    pub(super) fn load(&self) -> LoadGeneration {
        match self {
            Self::Prepared { preparation, .. } => preparation.stamp().load(),
            Self::Waiting { load, .. } | Self::Replanning { load, .. } => *load,
        }
    }

    pub(super) fn transport(&self) -> TransportRevision {
        match self {
            Self::Prepared { preparation, .. } => preparation.stamp().transport(),
            Self::Waiting { transport, .. } | Self::Replanning { transport, .. } => *transport,
        }
    }

    /// Whether the decision moves its member onto a map. One waiting to be
    /// planned again still does, so it keeps an unpresented entry's prior
    /// timeline in custody.
    pub(super) fn enters_map(&self) -> bool {
        match self {
            Self::Prepared { preparation, .. } => {
                matches!(preparation.effect(), SyncEffect::Projection { .. })
            }
            Self::Waiting { .. } => false,
            Self::Replanning { .. } => true,
        }
    }

    pub(super) const fn preparation(&self) -> Option<&SyncPreparation> {
        match self {
            Self::Prepared { preparation, .. } => Some(preparation),
            Self::Waiting { .. } | Self::Replanning { .. } => None,
        }
    }

    pub(super) const fn relocates(&self) -> bool {
        matches!(
            self,
            Self::Prepared {
                entry: Entry::Relocate,
                ..
            }
        )
    }

    pub(super) const fn armed(&self) -> bool {
        matches!(
            self,
            Self::Prepared {
                phase: Phase::Armed,
                ..
            }
        )
    }
}

/// The preparations `next` issues and withdraws compared with `held`: each
/// one a member did not hold before is issued, and each one whose operation
/// no preparation of its member carries on afterwards is withdrawn.
pub(super) fn transition(held: &[Pending], next: &[Pending]) -> SyncTransition {
    let issued = next
        .iter()
        .filter_map(Pending::preparation)
        .filter(|preparation| {
            !held
                .iter()
                .any(|old| old.preparation() == Some(preparation))
        })
        .cloned()
        .collect();
    let withdrawn = held
        .iter()
        .filter_map(Pending::preparation)
        .filter(|preparation| {
            let member = preparation.stamp().member().grid_id();
            let operation = preparation.stamp().operation();
            !next.iter().any(|new| {
                new.preparation().is_some()
                    && new.member() == member
                    && new.operation() == operation
            })
        })
        .map(SyncPreparation::stamp)
        .collect();
    SyncTransition::new(issued, withdrawn)
}
