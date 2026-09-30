use kithara_effects::LimiterConfig;

use super::{LimiterNode, MetronomeConfig, MetronomeNode};

/// The session output chain after the mix: the limiter, then the metronome
/// whose click ducks the limited signal under it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SessionOutput {
    limiter: LimiterConfig,
    metronome: MetronomeConfig,
}

impl SessionOutput {
    pub(crate) const fn new(limiter: LimiterConfig, metronome: MetronomeConfig) -> Self {
        Self { limiter, metronome }
    }

    pub(crate) fn limiter(&self) -> LimiterNode {
        LimiterNode::new(self.limiter)
    }

    pub(crate) fn metronome(&self, enabled: bool) -> MetronomeNode {
        MetronomeNode::new(enabled, self.metronome, self.limiter.ceiling())
    }
}
