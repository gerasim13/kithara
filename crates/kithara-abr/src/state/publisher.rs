use kithara_platform::{sync::Arc, time::Instant};

use super::{AbrDecision, AbrState, AbrTicket, PendingAbrDecision};

/// Publication authority retained by the owner of an [`AbrState`].
///
/// Consumers receive [`crate::AbrHandle`] for observation and control. Only
/// the source owner carries this capability into its exact transition commit.
#[derive(Clone)]
pub struct AbrPublisher {
    state: Arc<AbrState>,
}

impl AbrPublisher {
    pub(super) fn new(state: Arc<AbrState>) -> Self {
        Self { state }
    }

    /// Apply the temporary target-only legacy publication contract.
    pub fn apply_legacy_decision(&self, decision: &AbrDecision, now: Instant) {
        self.state.apply_decision(decision, now);
    }

    /// Drop the pending request only when `ticket` still identifies it.
    #[must_use]
    pub fn abort_pending(&self, ticket: AbrTicket) -> bool {
        self.state.abort_pending(ticket)
    }

    /// Publish only the exact pending request represented by `claim`.
    #[must_use]
    pub fn commit_pending(&self, claim: PendingAbrDecision, now: Instant) -> bool {
        self.state.commit_pending(claim, now)
    }
}
