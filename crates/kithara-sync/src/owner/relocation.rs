use std::ops::Range;

use kithara_signal::{SessionFrame, TransportRevision};
use kithara_warp::{AssetFrame, BeatGridId, PresentationFrontier};

use super::{
    placement::{Missing, place},
    preparation::{Decision, Entry},
    state::GroupState,
};
use crate::{AlignmentSource, LoadGeneration, SyncAdmission, SyncError, SyncGroup};

/// The request one relocation answers.
pub(super) struct RelocateRequest {
    pub(super) target: BeatGridId,
    pub(super) load: LoadGeneration,
    pub(super) transport: TransportRevision,
    pub(super) cue: AssetFrame,
    pub(super) frontier: PresentationFrontier,
    pub(super) window: Range<SessionFrame>,
}

impl<G: SyncGroup<NestedGroup = G>> GroupState<G> {
    /// Moves one direct grid member that sounds through its applied map to
    /// the exact recording `cue`, entering on the group's beats in the cue's
    /// own beat and bar phase no earlier than the frame after its presented
    /// frontier.
    ///
    /// The applied map keeps sounding until the relocation is presented, and
    /// the member's other pending decision is replaced only once the
    /// relocation is admitted. A cue the member grid does not cover yet is
    /// refused rather than left waiting.
    pub(super) fn transact_relocate(
        &mut self,
        request: RelocateRequest,
    ) -> Result<SyncAdmission, SyncError> {
        let RelocateRequest {
            target,
            load,
            transport,
            cue,
            frontier,
            window,
        } = request;
        let member = self.admissible_member(target)?;
        let lane = self
            .applied_of(target)
            .ok_or(SyncError::MemberSilent { member_id: target })?;
        if frontier.warp_map() != Some(lane.map()) {
            return Err(SyncError::AudibleMapMismatch {
                member_id: target,
                expected: Some(lane.map()),
                given: frontier.warp_map(),
            });
        }
        let sounding = lane.applied().stamp().load();
        if load != sounding {
            return Err(SyncError::LoadMismatch {
                member_id: target,
                expected: sounding,
                given: load,
            });
        }
        let replaces = Some(lane.map());
        let past_frontier = i64::from(frontier.output())
            .checked_add(1)
            .map(SessionFrame::new)
            .ok_or(SyncError::OutsideGrid { grid_id: target })?;
        let first = window.start.max(past_frontier);
        if first >= window.end {
            return Err(SyncError::NoAdmissibleBoundary {
                member_id: target,
                first,
                end: window.end,
            });
        }
        let placement = place(
            &self.grid,
            &member,
            AlignmentSource::Cued(cue),
            &(first..window.end),
        )
        .map_err(|missing| match missing {
            Missing::Coverage(_) => {
                Missing::Refused(SyncError::RelocationUncovered { member_id: target })
            }
            refused @ Missing::Refused(_) => refused,
        });
        self.admit(
            Decision {
                member,
                load,
                transport,
                entry: Entry::Relocate,
                replaces,
            },
            placement,
        )
    }
}
