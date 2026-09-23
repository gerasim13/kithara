use std::ops::Range;

use kithara_signal::{SessionFrame, TransportRevision};
use kithara_warp::{BeatGridId, MapRegion};

use super::{
    placement::{Missing, place, project},
    state::GroupState,
    timeline::Timeline,
    transaction::take_operation,
};
use crate::{
    AlignmentSource, LoadGeneration, SyncAdmission, SyncCapability, SyncError, SyncExecutionStamp,
    SyncGroup, SyncMember, SyncOperationId, SyncPreparation,
};

/// The one unapplied decision a group holds for a direct member.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Pending {
    /// The member's preparation waits for its executor.
    Prepared(SyncPreparation),
    /// The member's preparation needs grid coverage not yet published.
    Waiting {
        member: BeatGridId,
        operation: SyncOperationId,
        required: MapRegion,
    },
}

impl Pending {
    pub(super) fn member(&self) -> BeatGridId {
        match self {
            Self::Prepared(preparation) => preparation.stamp().member().grid_id(),
            Self::Waiting { member, .. } => *member,
        }
    }

    pub(super) fn operation(&self) -> SyncOperationId {
        match self {
            Self::Prepared(preparation) => preparation.stamp().operation(),
            Self::Waiting { operation, .. } => *operation,
        }
    }
}

/// The request one preparation answers.
pub(super) struct PrepareRequest {
    pub(super) target: BeatGridId,
    pub(super) load: LoadGeneration,
    pub(super) transport: TransportRevision,
    pub(super) source: AlignmentSource,
    pub(super) window: Range<SessionFrame>,
}

impl<G: SyncGroup<NestedGroup = G>> GroupState<G> {
    /// Prepares one direct grid member to enter this group's beat timeline.
    ///
    /// A preparation replaces only the member's own pending decision. A
    /// refusal changes nothing and spends no operation identity; a missing
    /// coverage spends one, so the wait it reports stays addressable.
    pub(super) fn transact_prepare(
        &mut self,
        request: PrepareRequest,
    ) -> Result<SyncAdmission, SyncError> {
        let PrepareRequest {
            target,
            load,
            transport,
            source,
            window,
        } = request;
        if matches!(self.timeline, Timeline::Off) {
            return Err(SyncError::CapabilityUnavailable {
                capability: SyncCapability::Alignment,
            });
        }
        let member = self
            .members
            .iter()
            .find_map(|member| match member {
                SyncMember::Grid { grid, .. } if grid.id() == target => Some(grid.snapshot()),
                SyncMember::Grid { .. } | SyncMember::Group { .. } => None,
            })
            .ok_or_else(|| SyncError::MemberNotFound {
                group_id: self.grid.id(),
                member_id: target,
            })?;
        let map_revision = self
            .next_map
            .ok_or_else(|| SyncError::WarpMapRevisionExhausted {
                group_id: self.grid.id(),
            })?;
        let planned = place(&self.grid, &member, source, &window)
            .and_then(|placement| project(&self.grid, &member, placement, map_revision));
        let planned = match planned {
            Ok(planned) => Ok(planned),
            Err(Missing::Coverage(required)) => Err(required),
            Err(Missing::Refused(error)) => return Err(error),
        };
        let operation = take_operation(self.grid.id(), &mut self.next_operation)?;
        let topology = self.topology_stamp();
        let (pending, admission) = match planned {
            Ok((alignment, plan)) => {
                self.next_map = map_revision.checked_next();
                let preparation = SyncPreparation::projection(
                    SyncExecutionStamp::new(
                        operation,
                        member.stamp(),
                        self.grid.stamp(),
                        topology,
                        load,
                        transport,
                    ),
                    alignment,
                    plan,
                );
                (
                    Pending::Prepared(preparation.clone()),
                    SyncAdmission::Prepared(preparation),
                )
            }
            Err(required) => (
                Pending::Waiting {
                    member: target,
                    operation,
                    required,
                },
                SyncAdmission::Deferred {
                    operation,
                    topology,
                    required,
                },
            ),
        };
        self.pending.retain(|held| held.member() != target);
        self.pending.push(pending);
        Ok(admission)
    }
}
