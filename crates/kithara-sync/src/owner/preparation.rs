use std::ops::Range;

use kithara_signal::{SessionFrame, TransportRevision};
use kithara_warp::{
    BeatAlignment, BeatGridId, BeatGridSnapshot, BeatGridState, MapRegion, WarpMapRevision,
    WarpPlan,
};

use super::{
    placement::{Missing, carry, place, project},
    state::GroupState,
    timeline::Timeline,
    transaction::take_operation,
};
use crate::{
    AlignmentSource, LoadGeneration, SyncAdmission, SyncCapability, SyncEffect, SyncError,
    SyncExecutionStamp, SyncGroup, SyncMember, SyncOperationId, SyncPreparation, TopologyStamp,
};

/// The one unapplied decision a group holds for a direct member.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Pending {
    /// The member's preparation waits for its executor, and must keep its
    /// activation inside the launch window it was asked for.
    Prepared {
        preparation: SyncPreparation,
        window: Range<SessionFrame>,
    },
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
            Self::Prepared { preparation, .. } => preparation.stamp().member().grid_id(),
            Self::Waiting { member, .. } => *member,
        }
    }

    pub(super) fn operation(&self) -> SyncOperationId {
        match self {
            Self::Prepared { preparation, .. } => preparation.stamp().operation(),
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

/// The pending decisions of every direct member on a successor group grid,
/// computed before anything changes.
pub(super) struct Refreshed {
    pub(super) pending: Vec<Pending>,
    pub(super) next_map: Option<WarpMapRevision>,
}

/// The facts one preparation is minted from, besides its placement.
struct Mint<'a> {
    owner: &'a BeatGridSnapshot,
    member: &'a BeatGridSnapshot,
    operation: SyncOperationId,
    topology: TopologyStamp,
    load: LoadGeneration,
    transport: TransportRevision,
    window: Range<SessionFrame>,
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
            .direct_grid(target)
            .ok_or_else(|| SyncError::MemberNotFound {
                group_id: self.grid.id(),
                member_id: target,
            })?;
        let mut next_map = self.next_map;
        let planned = place(&self.grid, &member, source, &window).and_then(|placement| {
            project(
                &self.grid,
                &member,
                placement,
                map_revision(&self.grid, next_map)?,
            )
        });
        let planned = match planned {
            Err(Missing::Refused(error)) => return Err(error),
            planned => planned,
        };
        let operation = take_operation(self.grid.id(), &mut self.next_operation)?;
        let topology = self.topology_stamp();
        let pending = Mint {
            owner: &self.grid,
            member: &member,
            operation,
            topology,
            load,
            transport,
            window,
        }
        .pending(planned, &mut next_map)?;
        let admission = match &pending {
            Pending::Prepared { preparation, .. } => SyncAdmission::Prepared(preparation.clone()),
            Pending::Waiting { required, .. } => SyncAdmission::Deferred {
                operation,
                topology,
                required: *required,
            },
        };
        self.next_map = next_map;
        self.pending.retain(|held| held.member() != target);
        self.pending.push(pending);
        Ok(admission)
    }

    /// Carries every pending decision onto the successor grid `grid` under
    /// `timeline`, without changing anything.
    ///
    /// A preparation keeps its operation and the beats that sound together
    /// and gets a new map on the new grid; one whose activation leaves its
    /// window, or whose member grid changed since, is withdrawn. A timeline
    /// without geometry, or on another axis, withdraws every decision.
    pub(super) fn refreshed(
        &self,
        grid: &BeatGridSnapshot,
        timeline: Timeline,
    ) -> Result<Refreshed, SyncError> {
        let mut next_map = self.next_map;
        if grid.stamp() == self.grid.stamp() {
            return Ok(Refreshed {
                pending: self.pending.clone(),
                next_map,
            });
        }
        if matches!(timeline, Timeline::Off)
            || grid.state() != BeatGridState::Live
            || grid.axis() != self.grid.axis()
        {
            return Ok(Refreshed {
                pending: Vec::new(),
                next_map,
            });
        }
        let mut pending: Vec<Pending> = Vec::with_capacity(self.pending.len());
        for held in &self.pending {
            let Pending::Prepared {
                preparation,
                window,
            } = held
            else {
                pending.push(held.clone());
                continue;
            };
            let stamp = preparation.stamp();
            let Some(member) = self
                .direct_grid(stamp.member().grid_id())
                .filter(|member| member.stamp() == stamp.member())
            else {
                continue;
            };
            let SyncEffect::Projection { alignment, .. } = preparation.effect();
            let placement = match carry(grid, &member, *alignment, window) {
                Ok(Some(placement)) => Ok(placement),
                Ok(None) => continue,
                Err(missing) => Err(missing),
            };
            let planned = placement.and_then(|placement| {
                project(grid, &member, placement, map_revision(grid, next_map)?)
            });
            let mint = Mint {
                owner: grid,
                member: &member,
                operation: stamp.operation(),
                topology: stamp.topology(),
                load: stamp.load(),
                transport: stamp.transport(),
                window: window.clone(),
            };
            pending.push(mint.pending(planned, &mut next_map)?);
        }
        Ok(Refreshed { pending, next_map })
    }

    /// Drops every pending decision whose member left the group or, for a
    /// preparation, no longer holds the grid it was projected from.
    pub(super) fn retain_current_pending(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        self.pending = pending
            .into_iter()
            .filter(|held| {
                self.direct_grid(held.member())
                    .is_some_and(|member| match held {
                        Pending::Prepared { preparation, .. } => {
                            member.stamp() == preparation.stamp().member()
                        }
                        Pending::Waiting { .. } => true,
                    })
            })
            .collect();
    }

    /// Returns the frozen grid of the direct grid member `id`.
    fn direct_grid(&self, id: BeatGridId) -> Option<BeatGridSnapshot> {
        self.members.iter().find_map(|member| match member {
            SyncMember::Grid { grid, .. } if grid.id() == id => Some(grid.snapshot()),
            SyncMember::Grid { .. } | SyncMember::Group { .. } => None,
        })
    }
}

impl Mint<'_> {
    /// Turns one planning result into the member's pending decision, spending
    /// the map revision a projection takes.
    fn pending(
        self,
        planned: Result<(BeatAlignment, WarpPlan), Missing>,
        next_map: &mut Option<WarpMapRevision>,
    ) -> Result<Pending, SyncError> {
        match planned {
            Ok((alignment, plan)) => {
                *next_map = plan.activation().revision().checked_next();
                Ok(Pending::Prepared {
                    preparation: SyncPreparation::projection(
                        SyncExecutionStamp::new(
                            self.operation,
                            self.member.stamp(),
                            self.owner.stamp(),
                            self.topology,
                            self.load,
                            self.transport,
                        ),
                        alignment,
                        plan,
                    ),
                    window: self.window,
                })
            }
            Err(Missing::Coverage(required)) => Ok(Pending::Waiting {
                member: self.member.id(),
                operation: self.operation,
                required,
            }),
            Err(Missing::Refused(error)) => Err(error),
        }
    }
}

/// The map revision the next projection on `owner` takes.
fn map_revision(
    owner: &BeatGridSnapshot,
    next_map: Option<WarpMapRevision>,
) -> Result<WarpMapRevision, Missing> {
    next_map.ok_or_else(|| {
        Missing::Refused(SyncError::WarpMapRevisionExhausted {
            group_id: owner.id(),
        })
    })
}
