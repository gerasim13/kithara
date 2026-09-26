use portable_atomic::{AtomicF32, Ordering};

/// The part of a player the Host holds: a web player stays on its own thread, so the
/// Host keeps only its level.
pub(crate) struct HeldPlayer(AtomicF32);

impl HeldPlayer {
    pub(crate) const fn new(level: f32) -> Self {
        Self(AtomicF32::new(level))
    }

    pub(crate) fn commit_host_level(&self, level: f32) {
        self.0.store(level, Ordering::Relaxed);
    }

    pub(crate) fn host_level(&self) -> f32 {
        self.0.load(Ordering::Relaxed)
    }
}
