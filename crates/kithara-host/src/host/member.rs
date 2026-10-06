use std::fmt;

use kithara_warp::BeatGridId;

/// What the Host holds of a deck's player. The native Host owns the player:
/// dropping it invalidates its controls, so it lives as long as its deck.
#[cfg(not(target_arch = "wasm32"))]
type HeldPlayer = Box<dyn kithara_play::player::Player>;
/// What the Host holds of a deck's player: a web player stays on its own
/// thread, held by the platform's residents, so the session holds nothing.
#[cfg(target_arch = "wasm32")]
type HeldPlayer = ();

/// One deck as the Host's session holds it: the identity its player
/// registered under, and the player while the deck is attached.
pub(crate) struct PlayerMember {
    grid_id: BeatGridId,
    _player: HeldPlayer,
}

impl PlayerMember {
    /// The deck a player registered as `grid_id`, the Host holding `player`
    /// of it.
    #[must_use]
    pub(crate) const fn new(grid_id: BeatGridId, player: HeldPlayer) -> Self {
        Self {
            grid_id,
            _player: player,
        }
    }

    /// The identity the deck registered under.
    #[must_use]
    pub(crate) const fn grid_id(&self) -> BeatGridId {
        self.grid_id
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
