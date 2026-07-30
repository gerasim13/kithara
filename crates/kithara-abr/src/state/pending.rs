use super::AbrDecision;

/// Identity of one accepted ABR switch request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AbrTicket(u64);

impl AbrTicket {
    pub(super) fn new(value: u64) -> Self {
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
    ticket: AbrTicket,
    decision: AbrDecision,
}

impl PendingAbrDecision {
    pub(super) fn new(ticket: AbrTicket, decision: AbrDecision) -> Self {
        Self { ticket, decision }
    }

    /// Return the decision captured by this claim.
    #[must_use]
    pub fn decision(self) -> AbrDecision {
        self.decision
    }

    /// Return the identity of the claimed request.
    #[must_use]
    pub fn ticket(self) -> AbrTicket {
        self.ticket
    }
}
