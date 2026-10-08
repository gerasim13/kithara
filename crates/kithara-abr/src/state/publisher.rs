use std::sync::atomic::Ordering;

use kithara_platform::{sync::Arc, time::Instant};

use super::{AbrDecision, AbrState, AbrTicket, PendingAbrDecision, pending::pending_decision};

/// Publication authority retained by the owner of an [`AbrState`].
///
/// Consumers receive [`crate::AbrHandle`] for observation and control. Only
/// the source owner carries this capability into its exact transition commit.
#[derive(Clone)]
pub struct AbrPublisher {
    state: Arc<AbrState>,
}

impl AbrPublisher {
    pub(super) const fn new(state: Arc<AbrState>) -> Self {
        Self { state }
    }

    delegate::delegate! {
        to self.state {
            /// Drop the pending request only when `ticket` still identifies it.
            #[must_use]
            pub fn abort_pending(&self, ticket: AbrTicket) -> bool;
            /// Publish only the exact pending request represented by `claim`.
            #[must_use]
            pub fn commit_pending(&self, claim: PendingAbrDecision, now: Instant) -> bool;
        }
    }
}

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

    /// Publish the switch: a [`AbrDecision::Stay`] is a no-op; otherwise
    /// atomically clear the pending slot **iff** it still references the
    /// same target as `decision`, then store `current_variant :=
    /// decision.target()` and record the switch timestamp.
    ///
    /// The "iff" rule preserves the replace-pending semantic: if an
    /// external `request_target` overwrote the slot with a different
    /// target between [`peek_pending_decision`](Self::peek_pending_decision)
    /// and this call, the new pending stays untouched and the next
    /// boundary commits it. The captured `decision` still publishes —
    /// the caller has already prepared `v_new` for that target.
    pub fn apply_decision(&self, decision: &AbrDecision, now: Instant) {
        if !decision.changed() {
            return;
        }
        let target = decision.target();
        let mut state = self.pending.lock();
        if state
            .pending
            .as_ref()
            .is_some_and(|pending| pending.target == target)
        {
            state.pending = None;
        }
        self.clear_escape();
        self.current_variant.store(target.get(), Ordering::Release);
        self.record_switch(now);
        drop(state);
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

    /// Mint the publication capability retained by this state's source owner.
    #[must_use]
    pub fn publisher(self: &Arc<Self>) -> AbrPublisher {
        AbrPublisher::new(Arc::clone(self))
    }
}
