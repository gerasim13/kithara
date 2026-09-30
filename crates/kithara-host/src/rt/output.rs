use kithara_effects::LimiterConfig;

use super::{LimiterNode, MetronomeNode, metronome::Duck};
use crate::PlayError;

/// The session output chain after the mix: the limiter, then the metronome
/// whose click ducks the limited signal under it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SessionOutput {
    limiter: LimiterConfig,
    duck: Duck,
}

impl SessionOutput {
    /// # Errors
    ///
    /// Returns [`PlayError::InvalidParameter`] unless
    /// `0 < metronome_level <= limiter.ceiling()`.
    pub(crate) fn new(limiter: LimiterConfig, metronome_level: f32) -> Result<Self, PlayError> {
        let duck = Duck::new(metronome_level, limiter.ceiling())?;
        Ok(Self { limiter, duck })
    }

    pub(crate) fn limiter(&self) -> LimiterNode {
        LimiterNode::new(self.limiter)
    }

    pub(crate) fn metronome(&self, enabled: bool) -> MetronomeNode {
        MetronomeNode::new(enabled, self.duck)
    }
}
