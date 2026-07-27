use std::{num::NonZeroU32, sync::atomic::AtomicU64};

use bon::Builder;
use kithara_bufpool::{BytePool, PcmPool};
use kithara_platform::{sync::Arc, time::Duration};
use kithara_resampler::{NoResamplerBackend, ResamplerBackend, ResamplerOptions, ResamplerQuality};
use kithara_stream::{BoxedEventSink, ByteMap};

use super::backend::DecoderBackend;
use crate::{GaplessMode, error::DecodeResult};

/// What the blender does when this decoder generation hands off to the next.
///
/// The decoder owns the policy so the player never duplicates or hard-codes
/// transition timing: whoever built the decoder chose it, and the blender
/// reads it back off the built decoder and matches on it. Both arms are the
/// same path — an instantaneous change is a blend of zero length, not a way
/// around the blender.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecoderBlend {
    /// Cut straight to the incoming generation on its first frame.
    Instant,
    /// Equal-gain crossfade over this much audio.
    ///
    /// Equal-gain rather than equal-power because the two generations are the
    /// same music decoded twice: they are correlated, so their sum is not root
    /// two, and an equal-power pair would peak three decibels high in the
    /// middle of every switch.
    Crossfade(Duration),
}

impl Default for DecoderBlend {
    /// 10 ms — long enough to mask the level and phase step between two
    /// independently decoded generations of the same content, short enough
    /// that no transient survives it.
    fn default() -> Self {
        Self::Crossfade(Duration::from_millis(10))
    }
}

impl From<Duration> for DecoderBlend {
    /// Zero is the instantaneous change, resolved here so no downstream code
    /// has to know that a zero-length crossfade means "cut".
    fn from(duration: Duration) -> Self {
        if duration.is_zero() {
            Self::Instant
        } else {
            Self::Crossfade(duration)
        }
    }
}

impl From<DecoderBlend> for Duration {
    fn from(blend: DecoderBlend) -> Self {
        match blend {
            DecoderBlend::Instant => Self::ZERO,
            DecoderBlend::Crossfade(duration) => duration,
        }
    }
}

/// What a decoder generation keeps in hand, and how it gives it up.
///
/// Two numbers because a decoder change is two problems. The lead is audio
/// already decoded and not yet played — what the listener hears while the next
/// generation is being built, and the only reason a rebuild can be silent
/// rather than heard. The blend is the last of that lead, spent fading into the
/// generation that replaces it rather than played straight.
///
/// A lead shorter than the blend would promise a ramp out of frames that were
/// never kept, so readers take the longer of the two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecoderHandoff {
    /// Audio decoded ahead of the listener and held.
    pub lead: Duration,
    /// How the incoming generation replaces this one.
    pub blend: DecoderBlend,
}

impl Default for DecoderHandoff {
    /// 250 ms of lead: long enough to cover fetching and building a decoder
    /// for the next variant over a live connection, short enough that a seek
    /// still fills it in one burst.
    fn default() -> Self {
        Self {
            lead: Duration::from_millis(250),
            blend: DecoderBlend::default(),
        }
    }
}

impl DecoderHandoff {
    /// Audio this generation holds back: the lead, never shorter than the ramp
    /// it has to end on.
    #[must_use]
    pub fn held(self) -> Duration {
        self.lead.max(Duration::from(self.blend))
    }
}

/// Decoder-side resampler selected by the caller.
///
/// This describes conversion that is part of decoder construction, not the
/// playback graph's effects chain. Backend choice is encoded by `B`.
#[derive(Clone, Builder)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct DecoderResamplerConfig<B = NoResamplerBackend> {
    pub backend: B,
    #[builder(default)]
    pub options: ResamplerOptions,
    #[builder(default)]
    pub quality: ResamplerQuality,
    pub target_sample_rate: NonZeroU32,
}

impl<B> DecoderResamplerConfig<B>
where
    B: ResamplerBackend,
{
    #[must_use]
    pub fn new(
        target_sample_rate: NonZeroU32,
        backend: B,
        quality: ResamplerQuality,
        options: ResamplerOptions,
    ) -> Self {
        Self {
            backend,
            options,
            quality,
            target_sample_rate,
        }
    }

    /// Build the decoder resampler config selected by the audio decoder config.
    ///
    /// # Errors
    ///
    /// Currently this constructor is infallible; it returns a `Result` so the
    /// typed Apple codec-integrated plan can add pair validation without
    /// widening call sites again.
    pub fn for_decoder_backend(
        _decoder_backend: DecoderBackend,
        target_sample_rate: NonZeroU32,
        backend: B,
        quality: ResamplerQuality,
        options: ResamplerOptions,
    ) -> DecodeResult<Option<Self>> {
        Ok(Some(Self::new(
            target_sample_rate,
            backend,
            quality,
            options,
        )))
    }
}

impl<B> std::fmt::Debug for DecoderResamplerConfig<B>
where
    B: ResamplerBackend,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecoderResamplerConfig")
            .field("backend", &self.backend.name())
            .field("options", &self.options)
            .field("quality", &self.quality)
            .field("target_sample_rate", &self.target_sample_rate)
            .finish()
    }
}

/// Configuration for `DecoderFactory`.
#[derive(Builder)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct DecoderConfig<B = NoResamplerBackend> {
    /// Which decoder backend to use. See [`DecoderBackend`].
    #[builder(default)]
    pub backend: DecoderBackend,
    /// What this decoder holds back, and how it hands off to the next
    /// generation.
    #[builder(default)]
    pub handoff: DecoderHandoff,
    /// Handle for dynamic byte length updates (HLS).
    pub byte_len_handle: Option<Arc<AtomicU64>>,
    /// Optional byte-map handle over the underlying source.
    pub byte_map: Option<Arc<dyn ByteMap>>,
    /// Raw byte buffer pool, propagated from the host.
    pub byte_pool: BytePool,
    /// File extension hint for Symphonia probe (e.g., "mp3", "aac").
    pub hint: Option<String>,
    /// Reader-side observer hooks. Single-owner; moved into
    /// [`ComposedDecoder`] by the chosen backend path.
    pub hooks: Option<BoxedEventSink>,
    /// PCM buffer pool, propagated from the host.
    pub pcm_pool: PcmPool,
    /// Optional decoder-side resampler plan. `None` means the decoder emits
    /// at the source rate.
    pub resampler: Option<DecoderResamplerConfig<B>>,
    /// How gapless trimming is resolved for this track. Drives the codec's own
    /// priming strip and the container gapless probe; the audio pipeline reads
    /// the same value to build its trimmer.
    #[builder(default)]
    pub gapless_mode: GaplessMode,
    /// Epoch counter for decoder recreation tracking.
    #[builder(default)]
    pub epoch: u64,
}

#[cfg(test)]
impl Default for DecoderConfig {
    fn default() -> Self {
        Self::builder()
            .byte_pool(BytePool::default())
            .pcm_pool(PcmPool::default())
            .build()
    }
}

#[cfg(test)]
mod tests {
    use kithara_resampler::rubato::RubatoBackend;

    use super::*;

    #[test]
    fn decoder_resampler_config_keeps_typed_backend() {
        let target_sample_rate = NonZeroU32::new(48_000).expect("test rate");
        let config: DecoderResamplerConfig<RubatoBackend> = DecoderResamplerConfig::builder()
            .target_sample_rate(target_sample_rate)
            .backend(RubatoBackend::default())
            .build();

        assert_eq!(config.target_sample_rate, target_sample_rate);
        assert_eq!(config.backend.name(), RubatoBackend::default().name());
    }
}
