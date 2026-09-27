use kithara_warp::BeatGridId;

use super::{InboxAt, RegisteredCell, RootCut, RootError, RootPort};
use crate::{
    ControlError, SyncError, SyncExecutionReject, SyncGroup, SyncOperation, SyncReceipt,
    SyncReceiptAck, SyncReceiptInbox,
};

impl<G: SyncGroup<NestedGroup = G>> RootCut<'_, G> {
    /// Records one receipt on the root group and publishes the result. An
    /// install mints its permit in the same reply. An install that raced a
    /// source change this cut already reconciled fails at the gate: its
    /// terminal rejection is kept, and the next cut withdraws the lane
    /// first, superseding that rejection.
    ///
    /// # Errors
    ///
    /// Returns [`RootError::InstallRacedSourceChange`] once that rejection is
    /// kept, or the refusal of the member cell or of the root group.
    pub fn acknowledge<P: RootPort<G>>(
        &mut self,
        port: &P,
        receipt: SyncReceipt,
    ) -> Result<SyncReceiptAck, RootError> {
        let permit = match receipt {
            SyncReceipt::Installed(stamp) => {
                let member = stamp.member().grid_id();
                let entry = self
                    .cells
                    .iter()
                    .find(|entry| entry.member() == member)
                    .ok_or(RootError::MemberNotRegistered(member))?;
                match self.control.mint_permit(&entry.cell, stamp) {
                    Ok(permit) => Some(permit),
                    Err(ControlError::SourceChanged) => {
                        queue_gate_failure(self.cells, receipt)?;
                        return Err(RootError::InstallRacedSourceChange);
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            SyncReceipt::Rejected { .. } | SyncReceipt::Armed(_) | SyncReceipt::Presented(_) => {
                None
            }
        };
        if let Err(error) = self.group.acknowledge(receipt)
            && !error.is_superseded_rejection(receipt)
        {
            return Err(error.into());
        }
        port.publish(self.group);
        Ok(permit.map_or(SyncReceiptAck::Recorded, SyncReceiptAck::Installed))
    }

    /// Brings the owner up to date before any work under this cut: audio
    /// receipts first, so a claimed preparation is presented; then committed
    /// source changes, so a decision placed on a changed source is withdrawn
    /// with its mode kept; then gate failures, which that withdrawal
    /// supersedes; then retiring slots the callback has handed back.
    pub(super) fn drain<P: RootPort<G>>(&mut self, port: &mut P) -> Result<(), RootError> {
        self.drain_audio(port)?;
        self.reconcile_sources(port)?;
        self.drain_gate_failures(port)?;
        self.reap(port)
    }

    /// Sole consumer of the per-slot callback receipts, of live slots and of
    /// retiring slots whose processor the callback may still run.
    fn drain_audio<P: RootPort<G>>(&mut self, port: &mut P) -> Result<(), RootError> {
        for deck in 0..port.decks() {
            for slot in 0..port.slots(deck) {
                self.drain_inbox(port, InboxAt::Live { deck, slot })?;
            }
        }
        for index in 0..port.retiring_len() {
            self.drain_inbox(port, InboxAt::Retiring(index))?;
        }
        Ok(())
    }

    /// A failed owner update keeps the popped receipt in its inbox for a
    /// later owner cut.
    fn drain_inbox<P: RootPort<G>>(&mut self, port: &mut P, at: InboxAt) -> Result<(), RootError> {
        while let Some(receipt) = port.inbox(at).and_then(SyncReceiptInbox::next_receipt) {
            let recorded = match receipt {
                SyncReceipt::Armed(_)
                | SyncReceipt::Presented(_)
                | SyncReceipt::Rejected { .. } => self.acknowledge(port, receipt).map(drop),
                SyncReceipt::Installed(_) => Err(RootError::NonAudioReceipt),
            };
            if let Err(error) = recorded {
                if let Some(inbox) = port.inbox(at) {
                    inbox.keep(receipt);
                }
                return Err(error);
            }
        }
        Ok(())
    }

    /// Withdraws what the owner planned against every member source a player
    /// has changed, then marks each change reconciled. A member the owner
    /// does not hold, before attach or after detach, has nothing to withdraw.
    /// A change committed after the read stays pending for the next
    /// reconciliation.
    fn reconcile_sources<P: RootPort<G>>(&mut self, port: &P) -> Result<(), RootError> {
        for entry in self.cells.iter() {
            let Some(observed) = self.control.source_change(&entry.cell) else {
                continue;
            };
            let member = entry.member();
            let revocation = self
                .control
                .preflight_revoke(&entry.cell)
                .map_err(SyncError::ExecutionControl)?;
            match self.group.transact(SyncOperation::InvalidateSource {
                target: member,
                change: observed.change(),
            }) {
                Ok(_) => {}
                Err(rejected)
                    if *rejected.error() == SyncError::GroupNotFound { group_id: member } => {}
                Err(rejected) => return Err(rejected.error().clone().into()),
            }
            revocation.revoke();
            self.control
                .acknowledge_source_change(&entry.cell, observed);
            port.publish(self.group);
        }
        Ok(())
    }

    fn drain_gate_failures<P: RootPort<G>>(&mut self, port: &P) -> Result<(), RootError> {
        for entry in self.cells.iter_mut() {
            let Some(receipt) = entry.pending_gate_receipt else {
                continue;
            };
            if let Err(error) = self.group.acknowledge(receipt)
                && !error.is_superseded_rejection(receipt)
            {
                return Err(error.into());
            }
            entry.pending_gate_receipt = None;
            port.publish(self.group);
        }
        Ok(())
    }

    /// Destroys the inbox of every retiring slot whose processor the
    /// callback has handed back. The producer is checked before the final
    /// drain, so no receipt it pushed is lost. A deck whose last processor
    /// is gone then withdraws only that track's remaining decisions. After
    /// that final drain every refusal of the withdrawal is a broken
    /// quiescence contract that no later drain can change, so it is reported
    /// once rather than retried.
    fn reap<P: RootPort<G>>(&mut self, port: &mut P) -> Result<(), RootError> {
        let mut index = 0;
        while index < port.retiring_len() {
            if !port
                .inbox(InboxAt::Retiring(index))
                .is_some_and(|inbox| inbox.is_producer_gone())
            {
                index += 1;
                continue;
            }
            self.drain_inbox(port, InboxAt::Retiring(index))?;
            let group = port.remove_retiring(index);
            if port.is_quiesced(group) {
                self.withdraw_quiesced(port, group)?;
            }
        }
        Ok(())
    }

    fn withdraw_quiesced<P: RootPort<G>>(
        &mut self,
        port: &P,
        group: BeatGridId,
    ) -> Result<(), RootError> {
        let Some(entry) = self.cells.iter().find(|entry| entry.group == group) else {
            return Ok(());
        };
        let revocation = self
            .control
            .preflight_revoke(&entry.cell)
            .map_err(SyncError::ExecutionControl)?;
        let _admission = self
            .group
            .transact(SyncOperation::WithdrawQuiescedMember {
                target: entry.member(),
            })
            .map_err(|rejected| RootError::Sync(rejected.error().clone()))?;
        revocation.revoke();
        port.publish(self.group);
        Ok(())
    }
}

/// Keeps the one terminal rejection a member's install earns when it reached
/// a busy owner, for the next cut to record after the audio claim in flight
/// completes. The executor drops its lane at once and cannot arm it.
pub(super) fn queue_gate_failure(
    cells: &mut [RegisteredCell],
    receipt: SyncReceipt,
) -> Result<(), RootError> {
    let (stamp, terminal) = match receipt {
        SyncReceipt::Installed(stamp) => (
            stamp,
            SyncReceipt::Rejected {
                stamp,
                reason: SyncExecutionReject::ControlBusy,
            },
        ),
        SyncReceipt::Rejected { stamp, .. } => (stamp, receipt),
        SyncReceipt::Armed(_) | SyncReceipt::Presented(_) => {
            return Err(RootError::AudioReceiptFromExecutor);
        }
    };
    let member = stamp.member().grid_id();
    let entry = cells
        .iter_mut()
        .find(|entry| entry.member() == member)
        .ok_or(RootError::MemberNotRegistered(member))?;
    match entry.pending_gate_receipt {
        Some(existing) if existing == terminal => Ok(()),
        Some(_) => Err(RootError::GateFailurePending),
        None => {
            entry.pending_gate_receipt = Some(terminal);
            Ok(())
        }
    }
}
