use kithara_play::player::{Player, ResidentLoadObservation};

/// The part of a player the Host holds: the native Host owns the player itself and
/// reads its level from it.
pub(crate) struct HeldPlayer(Box<dyn Player>);

impl HeldPlayer {
    pub(crate) fn new<P: Player>(player: P) -> Self {
        Self(Box::new(player))
    }

    delegate::delegate! {
        to self.0 {
            #[call(set_host_level)]
            pub(crate) fn commit_host_level(&self, level: f32);
            pub(crate) fn host_level(&self) -> f32;
            /// Observes the player's resident load afresh, outside its
            /// command admission.
            pub(crate) fn resident_observation(&self) -> Option<ResidentLoadObservation>;
        }
    }
}
