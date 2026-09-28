use kithara_events::TrackId;
use kithara_platform::atomic::RelaxedAtomicF32;
use kithara_sync::ResidentLoadObservation;

/// The part of a player the Host holds: a web player stays on its own thread, so the
/// Host keeps only its level.
pub(crate) struct HeldPlayer(RelaxedAtomicF32);

impl HeldPlayer {
    pub(crate) const fn new(level: f32) -> Self {
        Self(RelaxedAtomicF32::new(level))
    }

    delegate::delegate! {
        to self.0 {
            #[call(store)]
            pub(crate) fn commit_host_level(&self, level: f32);
            #[call(load)]
            pub(crate) fn host_level(&self) -> f32;
        }
    }

    /// The Host holds no web player it could observe, so a decision waiting
    /// for a fresh observation of its track ends.
    pub(crate) const fn resident_observation(&self) -> Option<ResidentLoadObservation<TrackId>> {
        None
    }
}
