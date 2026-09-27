use kithara_warp::{BeatGridId, BeatGridQuery, BeatGridStamp, WarpMapRevision, WarpPlan};
use num_traits::ToPrimitive;

use super::{
    pending::{Pending, Phase, transition},
    state::GroupState,
    timeline::{Custodian, Timeline},
    transaction::take_operation,
};
use crate::{
    SourceChange, SyncAdmission, SyncApplied, SyncEffect, SyncError, SyncExecutionReject,
    SyncExecutionStamp, SyncGroup, SyncMember, SyncOperationId, SyncPreparation, SyncReceipt,
    SyncStatusSnapshot, SyncTransition,
};

/// The map one direct member sounds through, as its executor presented it.
#[derive(Clone, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get, vis = "pub(super)")]
pub(super) struct Applied {
    /// The receipt that presented the map.
    #[field(get, copy)]
    applied: SyncApplied,
    /// The presented map and its activation.
    #[field(get)]
    plan: WarpPlan,
    /// How far the presented source lies from the map, in output frames.
    #[field(get, copy)]
    phase_error_frames: f64,
    /// The owner grid this unchanged presented map is proven to follow.
    #[field(get, copy)]
    locked_grid: BeatGridStamp,
}

/// The latest decision of a group that ended without sounding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Rejection {
    pub(super) operation: SyncOperationId,
    pub(super) reason: SyncExecutionReject,
}

/// What one receipt does to the preparation its member holds.
enum Step {
    Phase(Phase),
    Drop(SyncExecutionReject),
    Present(SyncApplied),
}

impl Applied {
    pub(super) fn member(&self) -> BeatGridId {
        self.applied.stamp().member().grid_id()
    }

    pub(super) fn map(&self) -> WarpMapRevision {
        self.plan.activation().revision()
    }

    /// A rejected first Host entry returns to the exact Local trajectory
    /// this map already followed; only that prior stamp may be carried over.
    pub(super) fn restore_local_lock(&mut self, prior: BeatGridStamp, restored: BeatGridStamp) {
        if self.locked_grid == prior {
            self.locked_grid = restored;
        }
    }
}

impl<G: SyncGroup<NestedGroup = G>> GroupState<G> {
    /// Withdraw one member after its last audio processor has left the callback.
    ///
    /// The Host has drained that processor's receipts before calling this. An
    /// Armed preparation therefore signals a broken receipt or quiescence
    /// contract and must not be discarded as an unplayed lane.
    ///
    /// # Errors
    /// Returns an error for an unknown member, an Armed preparation, or a
    /// prior timeline that cannot be restored.
    pub(super) fn withdraw_quiesced_member(
        &mut self,
        member: BeatGridId,
    ) -> Result<SyncTransition, SyncError> {
        if self.direct_grid(member).is_none() {
            return Err(SyncError::MemberNotFound {
                group_id: self.grid.id(),
                member_id: member,
            });
        }
        if self.members.len() != 1 {
            return Err(SyncError::QuiescedMemberNotSoleGrid {
                group_id: self.grid.id(),
                member_id: member,
            });
        }
        if let Some(pending) = self
            .pending
            .iter()
            .find(|pending| pending.member() == member && pending.armed())
        {
            return Err(SyncError::ArmedOperation {
                member_id: member,
                operation: pending.operation(),
            });
        }

        let restored =
            self.before_entry
                .filter(|(custodian, _)| match *custodian {
                    Custodian::Decision(operation) => self.pending.iter().any(|pending| {
                        pending.member() == member && pending.operation() == operation
                    }),
                    Custodian::Withdrawn(held) => held == member,
                })
                .map(|(_, prior)| self.restored_entry_grid(prior).map(|grid| (prior, grid)))
                .transpose()?;
        let mut remaining = self.pending.clone();
        remaining.retain(|pending| pending.member() != member);
        let transition = transition(&self.pending, &remaining);
        self.blocked = None;
        self.pending = remaining;
        self.applied.retain(|lane| lane.member() != member);
        if let Some((prior, grid)) = restored {
            for lane in &mut self.applied {
                lane.restore_local_lock(prior.grid(), grid.stamp());
            }
            self.timeline = prior.timeline();
            self.grid = grid;
            self.before_entry = None;
            self.blocked = None;
        }
        Ok(transition)
    }

    /// Withdraws every decision placed against one member's source after the
    /// player committed a change to that source.
    ///
    /// The accepted mode stays. An unpresented entry keeps its prior timeline
    /// in custody, because that timeline still sounds. A discontinuity also
    /// ends the applied map's proof of where the member stands; a timing
    /// change keeps it, because a mapped lane plays its map, not the speed.
    /// The Host has drained the member's receipts first, so an Armed
    /// preparation signals a broken receipt contract.
    ///
    /// # Errors
    /// Returns an error for an unknown member or an Armed preparation.
    pub(super) fn invalidate_source(
        &mut self,
        member: BeatGridId,
        change: SourceChange,
    ) -> Result<SyncTransition, SyncError> {
        if self.direct_grid(member).is_none() {
            return Err(SyncError::MemberNotFound {
                group_id: self.grid.id(),
                member_id: member,
            });
        }
        if let Some(pending) = self
            .pending
            .iter()
            .find(|pending| pending.member() == member && pending.armed())
        {
            return Err(SyncError::ArmedOperation {
                member_id: member,
                operation: pending.operation(),
            });
        }
        let mut remaining = self.pending.clone();
        remaining.retain(|pending| pending.member() != member);
        let transition = transition(&self.pending, &remaining);
        if let Some((custodian, prior)) = self.before_entry
            && self.pending.iter().any(|held| {
                held.member() == member && custodian == Custodian::Decision(held.operation())
            })
        {
            self.before_entry = Some((Custodian::Withdrawn(member), prior));
        }
        self.pending = remaining;
        if change == SourceChange::Discontinuity {
            self.applied.retain(|lane| lane.member() != member);
        }
        Ok(transition)
    }

    /// Records one executor receipt for a preparation this group issued to a
    /// direct member.
    ///
    /// A receipt advances its preparation one phase at a time: installed,
    /// armed, presented. Installing needs the member grid the preparation was
    /// placed on to still be current; what the executor holds already arms
    /// and sounds on a member grid refined since, and a Host deck then waits
    /// to catch up with the refinement. A rejection drops a preparation that
    /// is not armed and leaves the member on the map it already sounds
    /// through. A Host deck's entry or retarget that missed for a transient
    /// reason, or on a refined member grid, instead waits to be planned once
    /// more; a second miss ends it. A presentation makes the preparation's
    /// map the member's applied one, or releases the member for a handoff.
    /// Nothing changes on a refusal.
    pub(super) fn record(&mut self, receipt: SyncReceipt) -> Result<SyncStatusSnapshot, SyncError> {
        let stamp = receipt.stamp();
        let member = stamp.member().grid_id();
        let current = self
            .direct_grid(member)
            .ok_or_else(|| SyncError::MemberNotFound {
                group_id: self.grid.id(),
                member_id: member,
            })?
            .stamp();
        let held = self
            .pending
            .iter()
            .enumerate()
            .find_map(|(index, held)| match held {
                Pending::Prepared {
                    preparation,
                    entry,
                    phase,
                } if held.member() == member => {
                    Some((index, preparation, entry.replans_a_miss(), *phase))
                }
                Pending::Prepared { .. } | Pending::Waiting { .. } | Pending::Replanning { .. } => {
                    None
                }
            });
        let Some((index, preparation, replans_a_miss, phase)) = held else {
            return Err(if self.sounds(stamp) {
                SyncError::DuplicateAcknowledgement {
                    operation: stamp.operation(),
                }
            } else {
                SyncError::NoPreparedOperation
            });
        };
        let expected = preparation.stamp();
        let operation = expected.operation();
        if operation != stamp.operation() {
            return Err(if self.sounds(stamp) {
                SyncError::DuplicateAcknowledgement {
                    operation: stamp.operation(),
                }
            } else {
                SyncError::StaleAcknowledgement {
                    expected: operation,
                    given: stamp.operation(),
                }
            });
        }
        if expected != stamp {
            return Err(SyncError::ReceiptMismatch {
                expected: Box::new(expected),
                given: Box::new(stamp),
            });
        }
        let step = match (receipt, phase) {
            (SyncReceipt::Installed(_), Phase::Issued) => Step::Phase(Phase::Installed),
            (SyncReceipt::Armed(_), Phase::Installed) => Step::Phase(Phase::Armed),
            (SyncReceipt::Rejected { reason, .. }, Phase::Issued | Phase::Installed) => {
                Step::Drop(reason)
            }
            (SyncReceipt::Presented(applied), Phase::Armed) => Step::Present(applied),
            (SyncReceipt::Installed(_), Phase::Installed | Phase::Armed)
            | (SyncReceipt::Armed(_), Phase::Armed) => {
                return Err(SyncError::DuplicateAcknowledgement { operation });
            }
            _ => return Err(SyncError::ReceiptOutOfOrder { operation }),
        };
        let refined = current != expected.member();
        if matches!(step, Step::Phase(Phase::Installed)) && refined {
            return Err(SyncError::StaleGridRevision {
                current,
                given: expected.member(),
            });
        }
        let host_deck = matches!(self.timeline, Timeline::Host)
            && matches!(self.members.as_slice(), [SyncMember::Grid { .. }])
            && matches!(preparation.effect(), SyncEffect::Projection { .. });
        match step {
            Step::Phase(next) => {
                if let Some(Pending::Prepared { phase, .. }) = self.pending.get_mut(index) {
                    *phase = next;
                }
            }
            Step::Drop(reason) => {
                let transient = refined
                    || matches!(
                        reason,
                        SyncExecutionReject::Late | SyncExecutionReject::ControlBusy
                    );
                if host_deck && replans_a_miss && transient && self.replanned != Some(operation) {
                    if let Some(held) = self.pending.get_mut(index) {
                        *held = Pending::Replanning {
                            member,
                            operation,
                            load: expected.load(),
                            transport: expected.transport(),
                            missed: Some(reason),
                        };
                    }
                } else {
                    self.end_decision(index, operation)?;
                    self.rejection = Some(Rejection { operation, reason });
                }
            }
            Step::Present(applied) => {
                let lane = presented(preparation, applied)?;
                let mut next_operation = self.next_operation;
                let catch_up = (refined && host_deck && lane.is_some())
                    .then(|| take_operation(self.grid.id(), &mut next_operation))
                    .transpose()?;
                self.pending.remove(index);
                self.applied.retain(|held| held.member() != member);
                self.applied.extend(lane);
                if self
                    .before_entry
                    .is_some_and(|(held, _)| held == Custodian::Decision(operation))
                {
                    self.before_entry = None;
                }
                if let Some(catch_up) = catch_up {
                    self.next_operation = next_operation;
                    self.pending.push(Pending::Replanning {
                        member,
                        operation: catch_up,
                        load: expected.load(),
                        transport: expected.transport(),
                        missed: None,
                    });
                }
                self.rejection = None;
                self.replanned = None;
            }
        }
        Ok(self.status())
    }

    /// Ends the decision a deck holds while it waits to be planned again,
    /// because its Host cannot observe the track afresh. A missed decision
    /// ends rejected for the reason it missed with; a catch-up leaves the
    /// track sounding through the map it presented.
    pub(super) fn abandon_replan(
        &mut self,
        operation: SyncOperationId,
    ) -> Result<SyncAdmission, SyncError> {
        let (index, missed) = self
            .pending
            .iter()
            .enumerate()
            .find_map(|(index, held)| match held {
                Pending::Replanning {
                    operation: waiting,
                    missed,
                    ..
                } if *waiting == operation => Some((index, *missed)),
                Pending::Replanning { .. } | Pending::Prepared { .. } | Pending::Waiting { .. } => {
                    None
                }
            })
            .ok_or(SyncError::NotReplanning { operation })?;
        let reserved = self.reserve_operation()?;
        self.end_decision(index, operation)?;
        if let Some(reason) = missed {
            self.rejection = Some(Rejection { operation, reason });
        }
        self.next_operation = reserved.checked_next();
        Ok(SyncAdmission::StateChanged {
            operation: reserved,
            topology: self.topology_stamp(),
            mode: self.mode(),
            grid: self.grid.stamp(),
            transition: SyncTransition::default(),
        })
    }

    /// Ends the decision held at `index` without it sounding. An unpresented
    /// entry hands its group back the timeline that still sounds.
    fn end_decision(&mut self, index: usize, operation: SyncOperationId) -> Result<(), SyncError> {
        let restoration = match self.before_entry {
            Some((held, prior)) if held == Custodian::Decision(operation) => {
                Some((prior, self.restored_entry_grid(prior)?))
            }
            _ => None,
        };
        self.pending.remove(index);
        if let Some((prior, grid)) = restoration {
            for lane in &mut self.applied {
                lane.restore_local_lock(prior.grid(), grid.stamp());
            }
            self.timeline = prior.timeline();
            self.grid = grid;
            self.before_entry = None;
            self.blocked = None;
        }
        Ok(())
    }

    /// Whether `stamp` names the presentation a member already sounds through.
    fn sounds(&self, stamp: SyncExecutionStamp) -> bool {
        self.applied
            .iter()
            .any(|lane| lane.applied.stamp() == stamp)
    }

    pub(super) fn applied_of(&self, member: BeatGridId) -> Option<&Applied> {
        self.applied.iter().find(|lane| lane.member() == member)
    }
}

/// The lane `applied` leaves the member on once `preparation` sounds: its
/// projected map, or none after a handoff.
fn presented(
    preparation: &SyncPreparation,
    applied: SyncApplied,
) -> Result<Option<Applied>, SyncError> {
    let (warp_map, activation) = preparation.activation();
    let frontier = applied.frontier();
    let mismatch = || SyncError::PresentationMismatch {
        operation: preparation.stamp().operation(),
        expected: warp_map,
        given: frontier,
    };
    if frontier.warp_map() != warp_map || frontier.output() < activation {
        return Err(mismatch());
    }
    let plan = match preparation.effect() {
        SyncEffect::Projection { plan, .. } => plan,
        SyncEffect::Handoff { .. } => return Ok(None),
    };
    let (BeatGridQuery::Resolved(source), BeatGridQuery::Resolved(rate)) = (
        plan.source_at(frontier.output()),
        plan.rate_at(frontier.output()),
    ) else {
        return Err(mismatch());
    };
    let heard = frontier.source().to_f64().ok_or_else(mismatch)?;
    Ok(Some(Applied {
        applied,
        plan: plan.clone(),
        phase_error_frames: (heard - f64::from(source)) / rate,
        locked_grid: preparation.stamp().group(),
    }))
}
