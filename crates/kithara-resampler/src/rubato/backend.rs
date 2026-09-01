use kithara_bufpool::HasPool;

use super::{RubatoAlgorithm, RubatoConfig, resampler::RubatoResampler};
use crate::{ResamplerBackend, ResamplerBuildError, ResamplerCapabilities, ResamplerSettings};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RubatoBackend {
    config: RubatoConfig,
}

impl RubatoBackend {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            config: RubatoConfig {
                algorithm: RubatoAlgorithm::Async,
            },
        }
    }

    #[must_use]
    pub const fn with_config(config: RubatoConfig) -> Self {
        Self { config }
    }
}

impl ResamplerBackend for RubatoBackend {
    type Resampler = RubatoResampler;

    fn build<S>(
        &self,
        settings: &ResamplerSettings<S>,
    ) -> Result<Self::Resampler, ResamplerBuildError>
    where
        S: HasPool<f32>,
    {
        settings.validate(self)?;
        RubatoResampler::new(self.name(), self.config, settings)
    }

    fn capabilities(&self) -> ResamplerCapabilities {
        ResamplerCapabilities::FIXED_RATIO
            | ResamplerCapabilities::REPORTS_LATENCY
            | ResamplerCapabilities::STANDALONE
    }

    fn name(&self) -> &'static str {
        const BACKEND_RUBATO: &str = "rubato";

        BACKEND_RUBATO
    }
}
