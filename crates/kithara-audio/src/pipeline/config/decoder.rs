use std::num::NonZeroU32;

use kithara_config::Config;
use kithara_decode::{DecoderBackend, DecoderResamplerConfig, GaplessMode};
use kithara_derive::Patch;
use kithara_resampler::{NoResamplerBackend, ResamplerBackend, ResamplerOptions, ResamplerQuality};

#[derive(Clone, Debug, Config, fieldwork::Fieldwork)]
#[config(builder(state_mod(vis = "pub")))]
#[non_exhaustive]
#[fieldwork(get)]
pub struct DecoderResamplerSettings<B = NoResamplerBackend> {
    #[config(skip = "backend strategy selected by the caller")]
    pub(crate) backend: B,
    #[config(value, builder(default))]
    #[field(get(copy))]
    pub(crate) options: ResamplerOptions,
    #[config(value, builder(default))]
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

/// Decoder construction settings, including decoder-side resampling.
///
/// [`AudioDecoderConfigPatch`] is what a configuration document may say about
/// it, reached through `audio.decoder`.
#[derive(Clone, Debug, Config, fieldwork::Fieldwork, Patch)]
#[config(default, builder(state_mod(vis = "pub")))]
#[non_exhaustive]
#[fieldwork(opt_in, get)]
pub struct AudioDecoderConfig<B = NoResamplerBackend> {
    #[config(value, builder(default))]
    #[field(get, copy)]
    pub(crate) backend: DecoderBackend,
    #[config(value, builder(default))]
    #[field(get, copy)]
    pub(crate) gapless_mode: GaplessMode,
    /// Not a document key: `DecoderResamplerSettings` carries the resampler
    /// backend itself, an object the construction site hands over and no
    /// document can name. `None` means the decoder resamples through
    /// `B::default()` with this crate's own options and quality.
    #[config(skip = "caller-selected resampler backend strategy", patch(skip))]
    pub(crate) resampler: Option<DecoderResamplerSettings<B>>,
}

impl<B> AudioDecoderConfig<B>
where
    B: Default + ResamplerBackend,
{
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

    fn effective_resampler(&self) -> DecoderResamplerSettings<B> {
        self.resampler.clone().unwrap_or_default()
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
            pub const fn resampler(&self) -> Option<&DecoderResamplerSettings<B>>;
        }
    }
}
