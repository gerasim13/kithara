use std::num::NonZeroUsize;

use kithara_bufpool::{HasPool, PoolError, SampleBuffer};
use kithara_dsp::filter::{Biquad, FilterError};
use kithara_signal::{AudioChunk, AudioSpec};

use super::FilterConfig;
use crate::{AudioEffect, consts};

/// Why a filter's numerical state or pooled scratch could not be prepared.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PrepareError {
    #[error(transparent)]
    Filter(#[from] FilterError),
    #[error(transparent)]
    Pool(#[from] PoolError),
}

/// Causal filtering over bounded planar blocks in the existing effect chain.
///
/// Processing retains the DSP biquad's history and quiet-block reset policy.
/// Every input chunk is emitted immediately; flush emits no extra samples.
#[derive(kithara_config::ConfigOwner)]
#[config_owner(config)]
#[non_exhaustive]
pub struct FilterEffect<S> {
    spec: AudioSpec,
    config: FilterConfig<S>,
    filter: Biquad,
    planes: Vec<SampleBuffer>,
}

impl<S: HasPool<f32>> FilterEffect<S> {
    /// Prepare filter state and pooled scratch outside audio processing.
    ///
    /// # Errors
    /// Rejects zero channels, cutoff at/above Nyquist, unstable coefficients,
    /// backend setup failure, or a pool budget too small for the scratch.
    pub fn new(spec: AudioSpec, config: FilterConfig<S>) -> Result<Self, PrepareError> {
        let channels = NonZeroUsize::new(usize::from(spec.channels)).ok_or(FilterError::Shape)?;
        let coefficients = config.coefficients(f64::from(spec.sample_rate.get()))?;
        let mut filter = Biquad::new(channels, NonZeroUsize::MIN)?;
        filter.retune(0, coefficients)?;
        let planes = (0..channels.get())
            .map(|_| {
                config
                    .pools()
                    .get_with_len::<f32>(consts::FILTER_BLOCK_FRAMES)
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            spec,
            config,
            filter,
            planes,
        })
    }
}

impl<S: Send + Sync + 'static> AudioEffect for FilterEffect<S> {
    fn flush(&mut self) -> Option<AudioChunk> {
        None
    }

    fn held_source_frames(&self) -> u64 {
        0
    }

    fn process(&mut self, mut chunk: AudioChunk) -> Option<AudioChunk> {
        let channels = self.planes.len();
        if chunk.spec() != self.spec || !chunk.samples.len().is_multiple_of(channels) {
            return None;
        }
        for block in chunk
            .samples
            .chunks_mut(consts::FILTER_BLOCK_FRAMES * channels)
        {
            let frames = block.len() / channels;
            for (frame, input) in block.chunks_exact(channels).enumerate() {
                for (plane, sample) in self.planes.iter_mut().zip(input) {
                    plane[frame] = *sample;
                }
            }
            // Preparation fixes the plane count and capacity for every bounded block.
            self.filter
                .process(&mut self.planes, 0..frames)
                .expect("prepared filter planes cover the block");
            for (frame, output) in block.chunks_exact_mut(channels).enumerate() {
                for (sample, plane) in output.iter_mut().zip(&self.planes) {
                    *sample = plane[frame];
                }
            }
        }
        Some(chunk)
    }

    fn reset(&mut self) {
        self.filter.reset();
    }
}
