use std::sync::atomic::Ordering;

use super::{AbrDecision, AbrState};
use crate::{AbrMode, AbrReason, VariantIndex};

/// Identity of one accepted ABR switch request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AbrTicket(u64);

impl AbrTicket {
    pub(super) const fn new(value: u64) -> Self {
        Self(value)
    }
}

/// Observable state of the exact pending ABR claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PendingAbrClaim {
    /// No switch intent exists for the current audible variant.
    Absent,
    /// The intent exists but ABR publication is temporarily locked.
    Locked(PendingAbrDecision),
    /// The exact intent can be prepared or committed.
    Ready(PendingAbrDecision),
}

/// Read-only claim of one exact pending ABR decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct PendingAbrDecision {
    decision: AbrDecision,
    ticket: AbrTicket,
}

impl PendingAbrDecision {
    pub(super) const fn new(ticket: AbrTicket, decision: AbrDecision) -> Self {
        Self { decision, ticket }
    }

    /// Return the decision captured by this claim.
    #[must_use]
    pub const fn decision(self) -> AbrDecision {
        self.decision
    }

    /// Return the identity of the claimed request.
    #[must_use]
    pub const fn ticket(self) -> AbrTicket {
        self.ticket
    }
}

#[derive(Debug)]
pub(super) struct PendingState {
    pub(super) pending: Option<PendingApply>,
    next_ticket: u64,
}

impl Default for PendingState {
    fn default() -> Self {
        Self {
            next_ticket: 1,
            pending: None,
        }
    }
}

impl PendingState {
    /// Hand out the identity of one transition attempt. Two intents that
    /// share a ticket are indistinguishable to every holder of it, so a
    /// slot written for a new reason takes a new ticket.
    ///
    /// # Panics
    ///
    /// Panics after exhausting the monotonic ABR ticket space.
    pub(super) fn mint_ticket(&mut self) -> AbrTicket {
        assert!(self.next_ticket < u64::MAX, "ABR ticket space exhausted");
        let ticket = AbrTicket::new(self.next_ticket);
        self.next_ticket += 1;
        ticket
    }
}

/// Captured intent of a pending switch: the target variant index plus
/// the reason the requestor (controller, manual UI, scheduler) wants
/// recorded once the boundary commit lands.
#[derive(Clone, Copy, Debug)]
pub(super) struct PendingApply {
    pub(super) reason: AbrReason,
    pub(super) ticket: AbrTicket,
    pub(super) target: VariantIndex,
}

/// `true` for `AbrReason` variants that originate from a throughput
/// estimate, and so go stale when a fresher estimate disagrees. Manual and
/// first-pick reasons make no throughput claim.
const fn is_throughput_driven(reason: AbrReason) -> bool {
    matches!(
        reason,
        AbrReason::UpSwitch
            | AbrReason::DownSwitch
            | AbrReason::UrgentDownSwitch
            | AbrReason::EscapeStalled
    )
}

/// A switch the active variant's failure forced, rather than one taken to
/// improve quality. Its premise is that the variant stopped delivering, and
/// no amount of throughput evidence speaks to that: an estimate recovers as
/// soon as some *other* variant's segment lands, while the stalled one is
/// still stalled.
pub(super) const fn is_rescue(reason: AbrReason) -> bool {
    matches!(
        reason,
        AbrReason::EscapeStalled | AbrReason::UrgentDownSwitch
    )
}

/// Rebuild the typed [`AbrDecision`] a pending boundary-commit publishes,
/// using the live `from` and the reason captured at `request_target`. The
/// only reasons that reach a pending slot are switch-class (a `Stay` never
/// requests a target), so a non-down, non-manual reason classifies as an
/// up-switch.
pub(super) const fn pending_decision(
    from: VariantIndex,
    to: VariantIndex,
    reason: AbrReason,
) -> AbrDecision {
    match reason {
        AbrReason::ManualOverride => AbrDecision::Manual { from, to },
        AbrReason::DownSwitch | AbrReason::UrgentDownSwitch => {
            AbrDecision::DownSwitch { from, to, reason }
        }
        AbrReason::Initial
        | AbrReason::UpSwitch
        | AbrReason::MinInterval
        | AbrReason::NoEstimate
        | AbrReason::BufferTooLowForUpSwitch
        | AbrReason::EscapeStalled
        | AbrReason::AlreadyOptimal
        | AbrReason::Locked => AbrDecision::UpSwitch { from, to, reason },
    }
}

impl AbrState {
    /// Claim the exact pending request without consuming it.
    ///
    /// Returns `None` while locked, when no request exists, or when the
    /// pending target already equals `current`.
    #[must_use]
    pub fn claim_pending_decision(&self, current: VariantIndex) -> Option<PendingAbrDecision> {
        match self.pending_claim(current) {
            PendingAbrClaim::Ready(claim) => Some(claim),
            PendingAbrClaim::Absent | PendingAbrClaim::Locked(_) => None,
        }
    }

    /// Observe whether an exact pending intent is absent, temporarily locked,
    /// or ready to claim.
    #[must_use]
    pub fn pending_claim(&self, current: VariantIndex) -> PendingAbrClaim {
        let state = self.pending.lock();
        let Some(pending) = state.pending else {
            return PendingAbrClaim::Absent;
        };
        if pending.target == current {
            return PendingAbrClaim::Absent;
        }
        let claim = PendingAbrDecision::new(
            pending.ticket,
            pending_decision(current, pending.target, pending.reason),
        );
        let locked = self.is_locked();
        drop(state);
        if locked {
            return PendingAbrClaim::Locked(claim);
        }
        PendingAbrClaim::Ready(claim)
    }

    /// Phase 2 read-only view of the unobserved pending switch (if any).
    /// Used by the Phase 3 scheduler boundary check and by tests.
    #[must_use]
    pub fn pending_target(&self) -> Option<VariantIndex> {
        self.pending
            .lock()
            .pending
            .as_ref()
            .map(|pending| pending.target)
    }

    /// Replaces the pending boundary switch unless the same target is already queued.
    /// Manual mode accepts only its pinned target.
    ///
    /// # Panics
    ///
    /// Panics after exhausting the monotonic ABR ticket space.
    pub fn request_target(&self, target: VariantIndex, reason: AbrReason) {
        let mut state = self.pending.lock();
        if let AbrMode::Manual(idx) = self.mode()
            && idx != target
        {
            return;
        }
        if state
            .pending
            .is_some_and(|pending| pending.target == target)
        {
            return;
        }
        let ticket = state.mint_ticket();
        state.pending = Some(PendingApply {
            reason,
            ticket,
            target,
        });
    }

    /// Drop a throughput-driven pending whose target the live decision no
    /// longer wants. Called from the controller tick's
    /// `Stay { AlreadyOptimal }` arm — the one verdict that re-affirms
    /// `current` against fresh evidence. Without it, an urgent down-switch
    /// latched on the initial throughput seed outlives the estimate that
    /// justified it and commits a quality drop at the next boundary.
    ///
    /// Manual and first-pick intents are not throughput claims, and a rescue
    /// is not one either (see [`is_rescue`]). A pending that already targets
    /// `current` describes no divergence and is left untouched.
    pub(crate) fn retract_throughput_pending(&self, current: VariantIndex) {
        let mut state = self.pending.lock();
        if state.pending.as_ref().is_some_and(|p| {
            is_throughput_driven(p.reason) && !is_rescue(p.reason) && p.target != current
        }) {
            state.pending = None;
        }
    }

    /// Applies a validated mode and clears any superseded pending switch.
    ///
    /// A manual pin on the target the slot already holds keeps that intent
    /// alive, but under a fresh ticket. The ticket is the identity of one
    /// transition attempt, and the slot it finds may already be claimed:
    /// reusing the ticket would let that attempt's abort cancel the command
    /// the listener has just given. Re-pinning what is already a manual
    /// override on that target restates nothing and keeps its ticket.
    ///
    /// # Panics
    ///
    /// Panics after exhausting the monotonic ABR ticket space.
    pub fn set_mode(&self, mode: AbrMode) {
        let mut state = self.pending.lock();
        self.mode.store(mode.into(), Ordering::Release);
        let restated = match mode {
            AbrMode::Manual(target) => state.pending.filter(|pending| pending.target == target),
            AbrMode::Auto(_) => None,
        };
        match restated {
            Some(pending) if matches!(pending.reason, AbrReason::ManualOverride) => {}
            Some(pending) => {
                let ticket = state.mint_ticket();
                state.pending = Some(PendingApply {
                    reason: AbrReason::ManualOverride,
                    ticket,
                    ..pending
                });
            }
            None => state.pending = None,
        }
    }

    delegate::delegate! {
        to self {
            /// Read-only peek at the pending decision. Returns the
            /// [`AbrDecision`] that [`apply_decision`](Self::apply_decision)
            /// would publish, or `None` when:
            /// - pending slot is empty;
            /// - state is locked (the seek-no-switch / blender invariant);
            /// - pending target equals `current` (no-op switch).
            ///
            /// Does not mutate. `current` is supplied by the caller to avoid a
            /// race with concurrent reads of [`current_variant_index`]; pass
            /// `self.current_variant_index()` if you do not need an externally
            /// pinned snapshot.
            #[must_use]
            #[expr($.map(PendingAbrDecision::decision))]
            #[call(claim_pending_decision)]
            pub fn peek_pending_decision(&self, current: VariantIndex) -> Option<AbrDecision>;
        }
    }
}
