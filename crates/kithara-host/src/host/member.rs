use std::fmt;

use kithara_warp::BeatGridId;

use super::HeldPlayer;

/// One deck as the Host's session holds it: the identity its player
/// registered under, and the deck's Host level.
pub(crate) struct PlayerMember {
    grid_id: BeatGridId,
    player: HeldPlayer,
}

impl PlayerMember {
    /// The deck a player registered as `grid_id`, the Host holding `player`
    /// of it.
    #[must_use]
    pub(crate) const fn new(grid_id: BeatGridId, player: HeldPlayer) -> Self {
        Self { grid_id, player }
    }

    /// The identity the deck registered under.
    #[must_use]
    pub(crate) const fn grid_id(&self) -> BeatGridId {
        self.grid_id
    }

    delegate::delegate! {
        to self.player {
            /// Commits the Host-applied level after its graph batch succeeds.
            pub(crate) fn commit_host_level(&self, level: f32);
            /// Reads the desired Host level used for later graph registration.
            #[must_use]
            pub(crate) fn host_level(&self) -> f32;
        }
    }
}

impl fmt::Debug for PlayerMember {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PlayerMember")
            .field("grid_id", &self.grid_id)
            .finish_non_exhaustive()
    }
}
