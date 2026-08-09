use std::num::NonZeroU32;

use bon::Builder;
use kithara_decode::{DecoderBackend, DecoderResamplerConfig, GaplessMode};
use kithara_resampler::{NoResamplerBackend, ResamplerBackend, ResamplerOptions, ResamplerQuality};

#[derive(Clone, Debug, Builder, fieldwork::Fieldwork)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
#[fieldwork(get)]
pub struct DecoderResamplerSettings<B = NoResamplerBackend> {
    pub(crate) backend: B,
    #[builder(default)]
    #[field(get(copy))]
    pub(crate) options: ResamplerOptions,
    #[builder(default)]
    #[field(get(copy))]
    pub(crate) quality: ResamplerQuality,
}

impl<B> Default for DecoderResamplerSettings<B>
where
    B: Default,
{
    fn default() -> Self {
        Self::builder().backend(B::default()).build()
    }
}

#[derive(Clone, Debug, Builder, fieldwork::Fieldwork)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
#[fieldwork(opt_in, get)]
pub struct AudioDecoderConfig<B = NoResamplerBackend> {
    #[builder(default)]
    #[field(get, copy)]
    pub(crate) backend: DecoderBackend,
    #[builder(default)]
    #[field(get, copy)]
    pub(crate) gapless_mode: GaplessMode,
    pub(crate) resampler: Option<DecoderResamplerSettings<B>>,
}

impl<B> AudioDecoderConfig<B>
where
    B: Default + ResamplerBackend,
{
    fn effective_resampler(&self) -> DecoderResamplerSettings<B> {
        self.resampler.clone().unwrap_or_default()
    }

    pub(crate) fn build_resampler_config(
        &self,
        target_sample_rate: Option<NonZeroU32>,
    ) -> Option<DecoderResamplerConfig<B>> {
        let target_sample_rate = target_sample_rate?;
        let resampler = self.effective_resampler();
        Some(
            DecoderResamplerConfig::builder()
                .target_sample_rate(target_sample_rate)
                .backend(resampler.backend)
                .quality(resampler.quality)
                .options(resampler.options)
                .build(),
        )
    }

    #[must_use]
    pub(crate) fn resampler_backend_name(&self) -> &'static str {
        self.effective_resampler().backend.name()
    }
}

impl<B> AudioDecoderConfig<B> {
    delegate::delegate! {
        to self.resampler {
            /// Return the explicitly configured decoder-side resampler settings.
            #[must_use]
            #[call(as_ref)]
            pub fn resampler(&self) -> Option<&DecoderResamplerSettings<B>>;
        }
    }
}

impl<B> Default for AudioDecoderConfig<B> {
    fn default() -> Self {
        Self::builder().build()
    }
}
