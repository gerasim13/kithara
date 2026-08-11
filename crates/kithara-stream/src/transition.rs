use kithara_abr::AbrTicket;
use kithara_events::{SeekEpoch, VariantIndex};

/// Result of publishing an audio-approved incoming variant.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum VariantPromotion {
    /// The exact ABR ticket and source session were published.
    Promoted,
    /// The transition is still exact, but publication is temporarily locked or
    /// its move-only reader has not been transferred yet.
    Deferred,
    /// The transition was superseded, aborted, promoted, or invalidated by a
    /// seek epoch change.
    Stale,
}

/// Exact identity of one variant transition in one seek epoch.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub struct VariantTransitionId {
    abr_ticket: AbrTicket,
    seek_epoch: SeekEpoch,
}

impl VariantTransitionId {
    /// Bind an accepted ABR request to the seek epoch that observed it.
    #[must_use]
    pub const fn new(abr_ticket: AbrTicket, seek_epoch: SeekEpoch) -> Self {
        Self {
            abr_ticket,
            seek_epoch,
        }
    }

    /// Accepted ABR request carried by this transition.
    #[must_use]
    pub const fn abr_ticket(self) -> AbrTicket {
        self.abr_ticket
    }

    /// Seek epoch in which the request was prepared.
    #[must_use]
    pub const fn seek_epoch(self) -> SeekEpoch {
        self.seek_epoch
    }
}

/// Fate of the outgoing source once this exact transition is accepted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum OutgoingDisposition {
    /// Keep the outgoing source available for the ordinary transition path.
    Retained,
    /// Stop relying on the outgoing source because it is no longer delivering.
    Abandoned,
}

/// Immutable route facts for one active-to-incoming transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, fieldwork::Fieldwork)]
#[non_exhaustive]
#[fieldwork(opt_in, with)]
pub struct VariantTransition {
    active_variant: VariantIndex,
    incoming_variant: VariantIndex,
    id: VariantTransitionId,
    #[field(with(
        vis = "pub",
        doc = "Return this transition with an explicitly bound outgoing disposition."
    ))]
    outgoing_disposition: OutgoingDisposition,
}

impl VariantTransition {
    /// Describe the exact source pair owned by one transition.
    #[must_use]
    pub const fn new(
        id: VariantTransitionId,
        active_variant: VariantIndex,
        incoming_variant: VariantIndex,
    ) -> Self {
        Self {
            active_variant,
            incoming_variant,
            id,
            outgoing_disposition: OutgoingDisposition::Retained,
        }
    }

    /// Variant that remains authoritative until promotion.
    #[must_use]
    pub const fn active_variant(self) -> VariantIndex {
        self.active_variant
    }

    /// Exact transition identity.
    #[must_use]
    pub const fn id(self) -> VariantTransitionId {
        self.id
    }

    /// Variant being prepared independently.
    #[must_use]
    pub const fn incoming_variant(self) -> VariantIndex {
        self.incoming_variant
    }

    /// Fate of the outgoing source for this exact transition.
    #[must_use]
    pub const fn outgoing_disposition(self) -> OutgoingDisposition {
        self.outgoing_disposition
    }
}

#[cfg(test)]
mod tests {
    use kithara_abr::{AbrState, PendingAbrDecision};
    use kithara_events::{AbrMode, AbrReason, VariantIndex};
    use kithara_test_utils::kithara;

    use super::*;

    fn ticket_for(target: usize) -> AbrTicket {
        let state = AbrState::new(AbrMode::Auto(Some(VariantIndex::new(0))));
        state.request_target(VariantIndex::new(target), AbrReason::UpSwitch);
        state
            .claim_pending_decision(VariantIndex::new(0))
            .map(PendingAbrDecision::ticket)
            .expect("requested target must produce an exact ticket")
    }

    #[kithara::test]
    fn transition_identity_includes_ticket_and_seek_epoch() {
        let ticket = ticket_for(1);
        let first = VariantTransitionId::new(ticket, 7);
        let same = VariantTransitionId::new(ticket, 7);
        let after_seek = VariantTransitionId::new(ticket, 8);

        assert_eq!(first, same);
        assert_ne!(first, after_seek);
    }

    #[kithara::test]
    fn transition_keeps_active_and_incoming_roles_distinct() {
        let transition = VariantTransition::new(
            VariantTransitionId::new(ticket_for(1), 3),
            VariantIndex::new(0),
            VariantIndex::new(1),
        );

        assert_eq!(transition.active_variant(), VariantIndex::new(0));
        assert_eq!(transition.incoming_variant(), VariantIndex::new(1));
        assert_eq!(transition.id().seek_epoch(), 3);
    }

    #[kithara::test]
    fn outgoing_disposition_defaults_to_retained_and_changes_immutably() {
        let retained = VariantTransition::new(
            VariantTransitionId::new(ticket_for(1), 3),
            VariantIndex::new(0),
            VariantIndex::new(1),
        );

        let abandoned = retained.with_outgoing_disposition(OutgoingDisposition::Abandoned);

        assert_eq!(
            retained.outgoing_disposition(),
            OutgoingDisposition::Retained
        );
        assert_eq!(
            abandoned.outgoing_disposition(),
            OutgoingDisposition::Abandoned
        );
        assert_eq!(abandoned.id(), retained.id());
    }
}
