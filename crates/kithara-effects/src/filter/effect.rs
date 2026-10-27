use std::num::NonZeroUsize;

use kithara_dsp::filter::{Biquad, FilterError};
use kithara_signal::{AudioChunk, AudioSpec};

use super::FilterConfig;
use crate::AudioEffect;

struct Channel {
    filter: Biquad,
    quiet_frames: u8,
}

/// Prepared causal filtering in the producer's existing [`AudioEffect`] chain.
///
/// Processing emits each chunk immediately and flush emits no extra samples.
/// Two consecutive input/output frames at or below `1e-9` clear that channel's
/// history. This tail rule is independent of caller chunk boundaries.
#[derive(kithara_config::ConfigOwner)]
#[config_owner(config)]
#[non_exhaustive]
pub struct FilterEffect {
    spec: AudioSpec,
    config: FilterConfig,
    channels: Vec<Channel>,
}

impl FilterEffect {
    /// Prepare channel state outside audio processing.
    ///
    /// # Errors
    /// Rejects zero channels, a cutoff at/above Nyquist, and designs rejected
    /// by the DSP owner's finite-coefficient and pole-stability validation.
    pub fn new(spec: AudioSpec, config: FilterConfig) -> Result<Self, FilterError> {
        if spec.channels == 0 {
            return Err(FilterError::Shape);
        }
        let coefficients = config.coefficients(f64::from(spec.sample_rate.get()))?;
        let channels = (0..spec.channels)
            .map(|_| {
                let mut filter = Biquad::new(NonZeroUsize::MIN, NonZeroUsize::MIN)?;
                filter.retune(0, coefficients)?;
                Ok(Channel {
                    filter,
                    quiet_frames: 0,
                })
            })
            .collect::<Result<_, FilterError>>()?;
        Ok(Self {
            spec,
            config,
            channels,
        })
    }
}

impl AudioEffect for FilterEffect {
    fn flush(&mut self) -> Option<AudioChunk> {
        None
    }

    fn held_source_frames(&self) -> u64 {
        0
    }

    /// Rejects a different format or incomplete interleaved frame without
    /// advancing history. Accepted chunks retain all timeline metadata.
    fn process(&mut self, mut chunk: AudioChunk) -> Option<AudioChunk> {
        let channels = self.channels.len();
        if chunk.spec() != self.spec || !chunk.samples.len().is_multiple_of(channels) {
            return None;
        }
        for frame in chunk.samples.chunks_exact_mut(channels) {
            for (sample, channel) in frame.iter_mut().zip(&mut self.channels) {
                let input = sample.abs();
                channel
                    .filter
                    .process(&mut [std::slice::from_mut(sample)], 0..1)
                    .ok()?;
                if input.max(sample.abs()) <= 1e-9 {
                    channel.quiet_frames += 1;
                    if channel.quiet_frames == 2 {
                        channel.filter.reset();
                        channel.quiet_frames = 0;
                    }
                } else {
                    channel.quiet_frames = 0;
                }
            }
        }
        Some(chunk)
    }

    fn reset(&mut self) {
        for channel in &mut self.channels {
            channel.filter.reset();
            channel.quiet_frames = 0;
        }
    }
}
