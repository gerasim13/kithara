use kithara_effects::LimiterConfig;

use super::{
    LimiterNode, MetronomeNode,
    metronome::{MetronomeConfig, MetronomeConfigLevelUpdate, MetronomeConfigUpdate},
};
use crate::RenderError;

/// The session output chain after the mix: the limiter, then the metronome
/// whose click ducks the limited signal under it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SessionOutput {
    limiter: LimiterConfig,
    metronome: MetronomeConfig,
}

impl SessionOutput {
    /// The output chain a session starts with.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::InvalidParameter`] naming the first metronome
    /// field out of its bounds.
    pub fn new(limiter: LimiterConfig, metronome: MetronomeConfig) -> Result<Self, RenderError> {
        Ok(Self {
            limiter,
            metronome: metronome.validated()?,
        })
    }

    #[must_use]
    pub fn limiter(&self) -> LimiterNode {
        LimiterNode::new(self.limiter)
    }

    #[must_use]
    pub fn metronome(&self, enabled: bool) -> MetronomeNode {
        MetronomeNode::new(enabled, self.metronome, self.limiter.ceiling())
    }

    /// Keeps `level` for every metronome node built from here on.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::InvalidParameter`] when `level` is outside its bounds.
    pub fn set_metronome_level(&mut self, level: f32) -> Result<(), RenderError> {
        self.metronome.apply_update(MetronomeConfigUpdate {
            level: MetronomeConfigLevelUpdate::Set { value: level },
            ..MetronomeConfigUpdate::default()
        })
    }
}
