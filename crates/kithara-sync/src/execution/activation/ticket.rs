use super::PreparedFirst;
use crate::{ArmPermit, LoadGeneration, SyncGateBinding};

/// One load of a member's media: the Player's item and the generation it
/// entered its slot with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadedMedia<I> {
    item: I,
    load: LoadGeneration,
}

impl<I: Copy> LoadedMedia<I> {
    /// `item` as loaded at generation `load`.
    #[must_use]
    pub const fn new(item: I, load: LoadGeneration) -> Self {
        Self { item, load }
    }

    /// The Player's item.
    #[must_use]
    pub const fn item(&self) -> I {
        self.item
    }

    /// The generation the item entered its slot with.
    #[must_use]
    pub const fn load(&self) -> LoadGeneration {
        self.load
    }
}

/// The exact installed lane an executor hands one member's audio path: the
/// load it serves, the first frame it decoded at its head, and the permit
/// its activation is claimed with through the member's gate.
pub struct SyncTicket<I, L> {
    media: LoadedMedia<I>,
    lane: L,
    first: PreparedFirst,
    permit: ArmPermit,
    gate: SyncGateBinding,
}

impl<I: Copy, L> SyncTicket<I, L> {
    /// The ticket of `lane`, loaded as `media`, entering with `first` once
    /// `permit` is claimed through `gate`.
    #[must_use]
    pub const fn new(
        media: LoadedMedia<I>,
        lane: L,
        first: PreparedFirst,
        permit: ArmPermit,
        gate: SyncGateBinding,
    ) -> Self {
        Self {
            media,
            lane,
            first,
            permit,
            gate,
        }
    }

    delegate::delegate! {
        to self.media {
            /// The Player's item the lane plays.
            #[must_use]
            pub fn item(&self) -> I;
            /// The load the lane serves.
            #[must_use]
            pub fn load(&self) -> LoadGeneration;
        }
    }

    /// The installed lane.
    #[must_use]
    pub const fn lane(&self) -> &L {
        &self.lane
    }

    /// The first frame the lane decoded at its head.
    #[must_use]
    pub const fn first(&self) -> PreparedFirst {
        self.first
    }

    /// The permit the owner minted for this installation.
    #[must_use]
    pub const fn permit(&self) -> ArmPermit {
        self.permit
    }

    /// The gate the activation is claimed through.
    #[must_use]
    pub const fn gate(&self) -> &SyncGateBinding {
        &self.gate
    }
}

/// The lane and its first frame, once the activation is claimed.
impl<I, L> From<SyncTicket<I, L>> for (L, PreparedFirst) {
    fn from(ticket: SyncTicket<I, L>) -> Self {
        (ticket.lane, ticket.first)
    }
}
