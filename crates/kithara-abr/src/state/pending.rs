use super::AbrDecision;

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
