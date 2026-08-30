use kithara_bufpool::HasPool;

use crate::{
    Resampler, ResamplerBackend, ResamplerBuildError, ResamplerCapabilities, ResamplerSettings,
};

const BACKEND_APPLE: &str = "apple-audio-converter";

pub trait AudioConverterFactory: Send + Sync + 'static {
    type Resampler: Resampler;

    /// Build a standalone PCM-to-PCM converter for the requested settings.
    ///
    /// # Errors
    ///
    /// Returns [`ResamplerBuildError`] when the platform converter cannot be
    /// constructed for the requested shape.
    fn build_resampler<S>(
        &self,
        settings: &ResamplerSettings<S>,
    ) -> Result<Self::Resampler, ResamplerBuildError>
    where
        S: HasPool<f32>;
}

#[derive(Clone, Debug, fieldwork::Fieldwork)]
#[non_exhaustive]
#[fieldwork(get)]
pub struct AppleAudioConverterBackend<F> {
    config: AppleAudioConverterConfig<F>,
}

#[derive(Clone, Debug, derive_more::From)]
#[non_exhaustive]
pub struct AppleAudioConverterConfig<F> {
    pub factory: F,
}

impl<F> AppleAudioConverterBackend<F> {
    #[must_use]
    pub const fn with_config(config: AppleAudioConverterConfig<F>) -> Self {
        Self { config }
    }
}

impl<F> ResamplerBackend for AppleAudioConverterBackend<F>
where
    F: AudioConverterFactory + Clone,
{
    type Resampler = F::Resampler;

    fn build<S>(
        &self,
        settings: &ResamplerSettings<S>,
    ) -> Result<Self::Resampler, ResamplerBuildError>
    where
        S: HasPool<f32>,
    {
        settings.validate(self)?;
        self.config.factory.build_resampler(settings)
    }

    fn capabilities(&self) -> ResamplerCapabilities {
        ResamplerCapabilities::FIXED_RATIO
            | ResamplerCapabilities::REPORTS_LATENCY
            | ResamplerCapabilities::STANDALONE
    }

    fn name(&self) -> &'static str {
        BACKEND_APPLE
    }
}
