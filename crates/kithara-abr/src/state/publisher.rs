use std::sync::atomic::Ordering;

use kithara_platform::time::Instant;

use super::{AbrState, AbrTicket, PendingAbrDecision, pending::pending_decision};

impl AbrState {
    /// Drop the pending request only when `ticket` still identifies it.
    ///
    /// Returns `true` when the matching request was removed. A stale ticket
    /// leaves a newer request untouched.
    #[must_use]
    pub fn abort_pending(&self, ticket: AbrTicket) -> bool {
        let mut state = self.pending.lock();
        if state
            .pending
            .as_ref()
            .is_none_or(|pending| pending.ticket != ticket)
        {
            return false;
        }
        state.pending = None;
        true
    }

    /// Publish `claim` only when it still identifies the pending request.
    ///
    /// Returns `true` after publishing and consuming the matching request. A
    /// stale claim leaves both the audible variant and any newer intent
    /// untouched.
    #[must_use]
    pub fn commit_pending(&self, claim: PendingAbrDecision, now: Instant) -> bool {
        let mut state = self.pending.lock();
        if self.is_locked() {
            return false;
        }
        let Some(pending) = state.pending else {
            return false;
        };
        let current = self.current_variant_index();
        let decision = pending_decision(current, pending.target, pending.reason);
        if pending.ticket != claim.ticket() || decision != claim.decision() {
            return false;
        }

        state.pending = None;
        let target = decision.target();
        self.clear_escape();
        self.current_variant.store(target.get(), Ordering::Release);
        self.record_switch(now);
        drop(state);
        true
    }
}
