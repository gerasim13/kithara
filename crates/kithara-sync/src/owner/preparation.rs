use std::ops::Range;

use kithara_signal::{SessionFrame, TransportRevision};
use kithara_warp::{
    BeatAlignment, BeatGridId, BeatGridQuery, BeatGridSnapshot, BeatGridState, MapRegion,
    WarpMapRevision, WarpPlan,
};

use super::{
    descent::Takeover,
    lifecycle::Applied,
    placement::{Missing, carry, continue_on, place, project},
    state::GroupState,
    timeline::Timeline,
    transaction::take_operation,
};
use crate::{
    AlignmentSource, LoadGeneration, SyncAdmission, SyncCapability, SyncEffect, SyncError,
    SyncExecutionStamp, SyncGroup, SyncMember, SyncOperationId, SyncPreparation, SyncTransition,
    TopologyStamp,
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
        required: MapRegion,
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

    const fn preparation(&self) -> Option<&SyncPreparation> {
        match self {
            Self::Prepared { preparation, .. } => Some(preparation),
            Self::Waiting { .. } => None,
        }
    }

    const fn armed(&self) -> bool {
        matches!(
            self,
            Self::Prepared {
                phase: Phase::Armed,
                ..
            }
        )
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

/// Every direct member's decisions on a successor group grid, computed
/// before anything changes.
pub(super) struct Refreshed {
    pub(super) pending: Vec<Pending>,
    pub(super) applied: Vec<Applied>,
    pub(super) next_map: Option<WarpMapRevision>,
    pub(super) next_operation: Option<SyncOperationId>,
}

/// The facts one preparation is minted from, besides its placement.
struct Mint<'a> {
    owner: &'a BeatGridSnapshot,
    member: &'a BeatGridSnapshot,
    operation: SyncOperationId,
    topology: TopologyStamp,
    load: LoadGeneration,
    transport: TransportRevision,
    entry: Entry,
    replaces: Option<WarpMapRevision>,
}

impl<G: SyncGroup<NestedGroup = G>> GroupState<G> {
    /// Prepares one direct grid member to enter this group's beat timeline,
    /// or a sounding one to continue on it.
    ///
    /// A preparation replaces only the member's own pending decision and
    /// leaves its applied map in place. A refusal changes nothing and spends
    /// no operation identity; a missing coverage spends one, so the wait it
    /// reports stays addressable.
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
        if let Some(held) = self.pending_of(target).filter(|held| held.armed()) {
            return Err(SyncError::ArmedOperation {
                member_id: target,
                operation: held.operation(),
            });
        }
        let lane = self.applied_of(target);
        let replaces = lane.map(Applied::map);
        let (entry, placement) = match (source, lane) {
            (AlignmentSource::Prepared(_) | AlignmentSource::Cued(_), Some(_)) => {
                return Err(SyncError::MemberAudible { member_id: target });
            }
            (AlignmentSource::Audible { frontier, .. }, _) if frontier.warp_map() != replaces => {
                return Err(SyncError::AudibleMapMismatch {
                    member_id: target,
                    expected: replaces,
                    given: frontier.warp_map(),
                });
            }
            (AlignmentSource::Audible { frontier, .. }, Some(lane)) => {
                let activation = window.start.max(frontier.output());
                if activation >= window.end {
                    return Err(SyncError::NoAdmissibleBoundary {
                        member_id: target,
                        first: activation,
                        end: window.end,
                    });
                }
                (
                    Entry::Replace,
                    continue_on(&self.grid, &member, lane.plan(), activation),
                )
            }
            (source, None) => (
                Entry::Launch(window.clone()),
                place(&self.grid, &member, source, &window),
            ),
        };
        let mut next_map = self.next_map;
        let planned = placement.and_then(|placement| {
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
            entry,
            replaces,
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

    /// Carries every direct member's decisions onto the successor `grid`
    /// under `timeline`, without changing anything.
    ///
    /// An armed preparation is kept as it is. A launch keeps its operation
    /// and the beats that sound together and gets a new map on the new grid;
    /// one whose activation leaves its window, or whose member grid changed
    /// since, is withdrawn. A sounding member is retargeted from the commit
    /// frame on, continuing the recording its applied map plays there. A
    /// timeline without geometry withdraws every decision but a handoff, and
    /// a new axis withdraws everything, applied maps too.
    pub(super) fn refreshed(
        &self,
        grid: &BeatGridSnapshot,
        timeline: Timeline,
        takeover: Takeover,
    ) -> Result<Refreshed, SyncError> {
        let Takeover {
            commit,
            mut next_operation,
        } = takeover;
        let mut next_map = self.next_map;
        if grid.stamp() == self.grid.stamp() {
            return Ok(Refreshed {
                pending: self.pending.clone(),
                applied: self.applied.clone(),
                next_map,
                next_operation,
            });
        }
        if grid.axis() != self.grid.axis() {
            return Ok(Refreshed {
                pending: Vec::new(),
                applied: Vec::new(),
                next_map,
                next_operation,
            });
        }
        let live = !matches!(timeline, Timeline::Off) && grid.state() == BeatGridState::Live;
        let mut pending: Vec<Pending> = Vec::with_capacity(self.pending.len());
        for member in self.members.iter().filter_map(|member| match member {
            SyncMember::Grid { grid, .. } => Some(grid.snapshot()),
            SyncMember::Group { .. } => None,
        }) {
            let held = self.pending_of(member.id());
            let decided = match (held, self.applied_of(member.id())) {
                (Some(held), _) if held.armed() => Some(held.clone()),
                (Some(held), _) if !live => handoff(held).cloned(),
                (_, Some(lane)) if live => {
                    let Some(commit) = commit else {
                        continue;
                    };
                    let operation = match held {
                        Some(held) => held.operation(),
                        None => take_operation(self.grid.id(), &mut next_operation)?,
                    };
                    let activation = commit.max(lane.applied().frontier().output());
                    let planned =
                        continue_on(grid, &member, lane.plan(), activation).and_then(|placement| {
                            project(grid, &member, placement, map_revision(grid, next_map)?)
                        });
                    let stamp = lane.applied().stamp();
                    let mint = Mint {
                        owner: grid,
                        member: &member,
                        operation,
                        topology: self.topology_stamp(),
                        load: stamp.load(),
                        transport: stamp.transport(),
                        entry: Entry::Replace,
                        replaces: Some(lane.map()),
                    };
                    Some(mint.pending(planned, &mut next_map)?)
                }
                (Some(waiting @ Pending::Waiting { .. }), None) => Some(waiting.clone()),
                (
                    Some(Pending::Prepared {
                        preparation,
                        entry: Entry::Launch(window),
                        ..
                    }),
                    None,
                ) if preparation.stamp().member() == member.stamp() => {
                    let SyncEffect::Projection { alignment, .. } = preparation.effect() else {
                        continue;
                    };
                    let planned = match carry(grid, &member, *alignment, window) {
                        Ok(Some(placement)) => map_revision(grid, next_map)
                            .and_then(|revision| project(grid, &member, placement, revision)),
                        Ok(None) => continue,
                        Err(missing) => Err(missing),
                    };
                    let stamp = preparation.stamp();
                    let mint = Mint {
                        owner: grid,
                        member: &member,
                        operation: stamp.operation(),
                        topology: stamp.topology(),
                        load: stamp.load(),
                        transport: stamp.transport(),
                        entry: Entry::Launch(window.clone()),
                        replaces: None,
                    };
                    Some(mint.pending(planned, &mut next_map)?)
                }
                _ => None,
            };
            pending.extend(decided);
        }
        Ok(Refreshed {
            pending,
            applied: self.applied.clone(),
            next_map,
            next_operation,
        })
    }

    /// Releases every sounding member of a group that leaves its timeline at
    /// `activation`: each one continues unsynchronized from the recording
    /// frame its applied map reaches there.
    pub(super) fn handoffs(
        &self,
        group: &BeatGridSnapshot,
        operation: SyncOperationId,
        transport: TransportRevision,
        activation: SessionFrame,
    ) -> Result<Vec<Pending>, SyncError> {
        if let Some(held) = self.pending.iter().find(|held| held.armed()) {
            return Err(SyncError::ArmedOperation {
                member_id: held.member(),
                operation: held.operation(),
            });
        }
        let mut handoffs: Vec<Pending> = Vec::with_capacity(self.applied.len());
        for lane in &self.applied {
            let member =
                self.direct_grid(lane.member())
                    .ok_or_else(|| SyncError::MemberNotFound {
                        group_id: self.grid.id(),
                        member_id: lane.member(),
                    })?;
            let BeatGridQuery::Resolved(source) = lane.plan().source_at(activation) else {
                return Err(SyncError::OutsideGrid {
                    grid_id: member.id(),
                });
            };
            let preparation = SyncPreparation::new(
                SyncExecutionStamp::new(
                    operation,
                    member.stamp(),
                    group.stamp(),
                    self.topology_stamp(),
                    lane.applied().stamp().load(),
                    transport,
                ),
                SyncEffect::Handoff {
                    replaces: lane.map(),
                    source,
                    activation,
                },
            );
            handoffs.push(Pending::Prepared {
                preparation,
                entry: Entry::Replace,
                phase: Phase::Issued,
            });
        }
        Ok(handoffs)
    }

    /// Drops, on a topology change, every decision whose member left the
    /// group and every issued or installed preparation: each was stamped with
    /// the topology being replaced, and an execution receipt is never
    /// restamped past a new fence. What is armed or applied already sounds,
    /// so a member that stays keeps it.
    pub(super) fn retain_current_pending(&mut self) -> SyncTransition {
        let held = std::mem::take(&mut self.pending);
        self.pending = held
            .iter()
            .filter(|held| {
                self.direct_grid(held.member()).is_some()
                    && !matches!(
                        held,
                        Pending::Prepared {
                            phase: Phase::Issued | Phase::Installed,
                            ..
                        }
                    )
            })
            .cloned()
            .collect();
        let applied = std::mem::take(&mut self.applied);
        self.applied = applied
            .into_iter()
            .filter(|lane| self.direct_grid(lane.member()).is_some())
            .collect();
        transition(&held, &self.pending)
    }

    /// Returns the frozen grid of the direct grid member `id`.
    pub(super) fn direct_grid(&self, id: BeatGridId) -> Option<BeatGridSnapshot> {
        self.members.iter().find_map(|member| match member {
            SyncMember::Grid { grid, .. } if grid.id() == id => Some(grid.snapshot()),
            SyncMember::Grid { .. } | SyncMember::Group { .. } => None,
        })
    }

    fn pending_of(&self, member: BeatGridId) -> Option<&Pending> {
        self.pending.iter().find(|held| held.member() == member)
    }
}

/// The preparations `next` issues and withdraws compared with `held`: each
/// one a member did not hold before is issued, and each one whose member
/// holds no preparation afterwards is withdrawn.
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
            !next
                .iter()
                .any(|new| new.preparation().is_some() && new.member() == member)
        })
        .map(SyncPreparation::stamp)
        .collect();
    SyncTransition::new(issued, withdrawn)
}

/// `held` when it still releases its member from a timeline that lost its
/// geometry: a handoff.
fn handoff(held: &Pending) -> Option<&Pending> {
    match held {
        Pending::Prepared { preparation, .. }
            if matches!(preparation.effect(), SyncEffect::Handoff { .. }) =>
        {
            Some(held)
        }
        Pending::Prepared { .. } | Pending::Waiting { .. } => None,
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
                    preparation: SyncPreparation::new(
                        SyncExecutionStamp::new(
                            self.operation,
                            self.member.stamp(),
                            self.owner.stamp(),
                            self.topology,
                            self.load,
                            self.transport,
                        ),
                        SyncEffect::Projection {
                            alignment,
                            plan,
                            replaces: self.replaces,
                        },
                    ),
                    entry: self.entry,
                    phase: Phase::Issued,
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
