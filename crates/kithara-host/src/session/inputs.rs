//! What the sync owner hears from outside its own transactions: executor
//! and audio receipts, and the source changes players committed.

use kithara_platform::sync::Arc;
use kithara_sync::{
    ArmPermit, ControlError, ControlGuard, SyncError, SyncExecutionReject, SyncGroup,
    SyncOperation, SyncReceipt, SyncReceiptAck,
};

use super::{protocol::SessionError, state::SessionState};

/// A timed-out Installed never reached the owner. The Host retains one
/// terminal rejection for that member, then applies it on the next Control
/// cut after the in-flight audio claim completes. The executor drops its lane
/// immediately and cannot accidentally arm it.
pub(super) fn queue_failed_gate_receipt<T, S>(
    state: &mut SessionState<T, S>,
    receipt: SyncReceipt,
) -> Result<(), SessionError> {
    let terminal = match receipt {
        SyncReceipt::Installed(stamp) => SyncReceipt::Rejected {
            stamp,
            reason: SyncExecutionReject::ControlBusy,
        },
        rejected @ SyncReceipt::Rejected { .. } => rejected,
        _ => return Err(SessionError::Graph("executor sent an audio receipt".into())),
    };
    let member = match terminal {
        SyncReceipt::Rejected { stamp, .. } => stamp.member().grid_id(),
        _ => {
            return Err(SessionError::Graph(
                "missing terminal rejection stamp".into(),
            ));
        }
    };
    let entry = state
        .sync_cells
        .iter_mut()
        .find(|entry| entry.cell.member() == member)
        .ok_or(SessionError::SyncMemberNotRegistered(member))?;
    match entry.pending_gate_receipt {
        Some(existing) if existing == terminal => Ok(()),
        Some(_) => Err(SessionError::Graph(
            "member already has an undrained gate failure".into(),
        )),
        None => {
            entry.pending_gate_receipt = Some(terminal);
            Ok(())
        }
    }
}

/// Bring the owner up to date before any work under a Control cut: audio
/// receipts first, so a claimed preparation is presented; then committed
/// source changes, so a decision placed on a changed source is withdrawn
/// with its mode kept; then gate failures, which that withdrawal supersedes.
pub(super) fn drain_owner_inputs<T, S>(
    state: &mut SessionState<T, S>,
    control: &ControlGuard<'_>,
) -> Result<(), SessionError> {
    drain_audio_receipts(state, control)?;
    reconcile_source_changes(state, control)?;
    drain_failed_gate_receipts(state)
}

/// Withdraw what the owner planned against every member source a player has
/// changed, then mark each change reconciled. A member the owner does not
/// hold, before attach or after detach, has nothing to withdraw. A change
/// committed after the read stays pending for the next reconciliation.
fn reconcile_source_changes<T, S>(
    state: &mut SessionState<T, S>,
    control: &ControlGuard<'_>,
) -> Result<(), SessionError> {
    for index in 0..state.sync_cells.len() {
        let cell = Arc::clone(&state.sync_cells[index].cell);
        let Some(observed) = control.source_change(&cell) else {
            continue;
        };
        let member = cell.member();
        let revocation = control
            .preflight_revoke(&cell)
            .map_err(SyncError::ExecutionControl)?;
        match state.root.transact(SyncOperation::InvalidateSource {
            target: member,
            change: observed.change(),
        }) {
            Ok(_) => {}
            Err(rejected) if *rejected.error() == SyncError::GroupNotFound { group_id: member } => {
            }
            Err(rejected) => return Err(SessionError::Sync(rejected.error().clone())),
        }
        revocation.revoke();
        control.acknowledge_source_change(&cell, observed);
        state.publish_root();
    }
    Ok(())
}

fn drain_failed_gate_receipts<T, S>(state: &mut SessionState<T, S>) -> Result<(), SessionError> {
    for index in 0..state.sync_cells.len() {
        let Some(receipt) = state.sync_cells[index].pending_gate_receipt else {
            continue;
        };
        if let Err(error) = state.root.acknowledge(receipt)
            && !error.is_superseded_rejection(receipt)
        {
            return Err(error.into());
        }
        state.sync_cells[index].pending_gate_receipt = None;
        state.publish_root();
    }
    Ok(())
}

/// Sole consumer of the per-slot callback receipts. A failed owner update
/// keeps the popped receipt in its fixed slot for a later owner cut.
fn drain_audio_receipts<T, S>(
    state: &mut SessionState<T, S>,
    control: &ControlGuard<'_>,
) -> Result<(), SessionError> {
    for deck_index in 0..state.graph.len() {
        let slots = state
            .graph
            .deck(deck_index)
            .map_or(0, |deck| deck.slots.len());
        for slot_index in 0..slots {
            loop {
                let receipt = state
                    .graph
                    .deck_mut(deck_index)
                    .and_then(|deck| deck.slots.get_mut(slot_index))
                    .and_then(|slot| {
                        slot.pending_receipt
                            .take()
                            .or_else(|| slot.sync_receipts.try_pop())
                    });
                let Some(receipt) = receipt else { break };
                let result = match receipt {
                    SyncReceipt::Armed(_)
                    | SyncReceipt::Presented(_)
                    | SyncReceipt::Rejected { .. } => {
                        acknowledge_root(state, receipt, control).map(|_| ())
                    }
                    _ => Err(SessionError::Graph(
                        "RT mailbox carried a non-audio sync receipt".into(),
                    )),
                };
                if let Err(error) = result {
                    if let Some(slot) = state
                        .graph
                        .deck_mut(deck_index)
                        .and_then(|deck| deck.slots.get_mut(slot_index))
                    {
                        slot.pending_receipt = Some(receipt);
                    }
                    return Err(error);
                }
            }
        }
    }
    Ok(())
}

pub(super) fn acknowledge_root<T, S>(
    state: &mut SessionState<T, S>,
    receipt: SyncReceipt,
    control: &ControlGuard<'_>,
) -> Result<SyncReceiptAck, SessionError> {
    let permit: Option<ArmPermit> = match receipt {
        SyncReceipt::Installed(stamp) => {
            let member = stamp.member().grid_id();
            let cell = state
                .sync_cells
                .iter()
                .find(|entry| entry.cell.member() == member)
                .ok_or(SessionError::SyncMemberNotRegistered(member))?;
            match control.mint_permit(&cell.cell, stamp) {
                Ok(permit) => Some(permit),
                // The player is changing this source. The lane fails at the
                // gate, and the change it waits for supersedes the rejection.
                Err(ControlError::SourceReserved | ControlError::SourceChanged) => {
                    queue_failed_gate_receipt(state, receipt)?;
                    return Err(SessionError::SyncControlBusy);
                }
                Err(error) => return Err(error.into()),
            }
        }
        _ => None,
    };
    if let Err(error) = state.root.acknowledge(receipt)
        && !error.is_superseded_rejection(receipt)
    {
        return Err(error.into());
    }
    state.publish_root();
    Ok(permit.map_or(SyncReceiptAck::Recorded, SyncReceiptAck::Installed))
}
