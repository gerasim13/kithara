use std::{fmt, num::NonZeroUsize};

use bungee_rs::Stream;
use fast_interleave::{deinterleave_variable, interleave_variable};
use kithara_bufpool::{BudgetExhausted, PcmPool};
use num_traits::{ToPrimitive, cast::AsPrimitive};

use crate::{
    ElasticCapabilities, ElasticConfig, ElasticEngine, ElasticError, ElasticLatency, ElasticRequest,
};

fn stream(
    sample_rate: u32,
    channels: usize,
    max_input_frames: usize,
) -> Result<Stream, ElasticError> {
    let sample_rate: usize = sample_rate.as_();
    Stream::new(sample_rate, channels, max_input_frames).map_err(ElasticError::EnginePreparation)
}

struct PooledPlanar {
    pool: PcmPool,
    channels: Vec<Vec<f32>>,
}

impl PooledPlanar {
    fn new(pool: &PcmPool, channels: usize, capacity: usize) -> Result<Self, BudgetExhausted> {
        let mut planar = Self {
            channels: Vec::with_capacity(channels),
            pool: pool.clone(),
        };
        for _ in 0..channels {
            let mut samples = pool.get();
            samples.ensure_len(capacity)?;
            samples.clear();
            planar.channels.push(samples.into_inner());
        }
        Ok(planar)
    }

    fn ensure_len(&mut self, frames: usize) -> Result<(), BudgetExhausted> {
        for samples in &mut self.channels {
            let mut pooled = self.pool.attach(std::mem::take(samples));
            let result = pooled.ensure_len(frames);
            *samples = pooled.into_inner();
            result?;
        }
        Ok(())
    }

    fn fill_interleaved(
        &mut self,
        input: &[f32],
        frames: usize,
        channels: NonZeroUsize,
    ) -> Result<(), BudgetExhausted> {
        self.ensure_len(frames)?;
        deinterleave_variable(input, channels, &mut self.channels, 0..frames);
        Ok(())
    }
}

impl AsRef<[Vec<f32>]> for PooledPlanar {
    fn as_ref(&self) -> &[Vec<f32>] {
        &self.channels
    }
}

impl AsMut<[Vec<f32>]> for PooledPlanar {
    fn as_mut(&mut self) -> &mut [Vec<f32>] {
        &mut self.channels
    }
}

impl Drop for PooledPlanar {
    fn drop(&mut self) {
        for samples in self.channels.drain(..) {
            self.pool.recycle(samples);
        }
    }
}

/// Exact-span Bungee engine.
///
/// Bungee has no history-only priming or true tail-drain operation, so this
/// engine implements neither [`crate::ElasticPriming`] nor terminal output in
/// [`ElasticEngine::flush`].
#[non_exhaustive]
pub struct BungeeElastic {
    stream: Stream,
    capabilities: ElasticCapabilities,
    source: PooledPlanar,
    output: PooledPlanar,
    pitch: f64,
}

impl BungeeElastic {
    const LATENCY_PROBE_BLOCKS: usize = 4;
    const LATENCY_PROBE_FRAMES: usize = 8192;

    fn pooled(
        pool: &PcmPool,
        channels: usize,
        frames: usize,
    ) -> Result<PooledPlanar, ElasticError> {
        PooledPlanar::new(pool, channels, frames).map_err(|_| ElasticError::PcmPoolBudgetExhausted)
    }

    fn latency(config: &ElasticConfig) -> Result<ElasticLatency, ElasticError> {
        let mut probe = stream(
            config.sample_rate(),
            config.channels(),
            Self::LATENCY_PROBE_FRAMES,
        )?;
        let mut source =
            Self::pooled(config.pool(), config.channels(), Self::LATENCY_PROBE_FRAMES)?;
        let mut output =
            Self::pooled(config.pool(), config.channels(), Self::LATENCY_PROBE_FRAMES)?;
        source
            .ensure_len(Self::LATENCY_PROBE_FRAMES)
            .map_err(|_| ElasticError::PcmPoolBudgetExhausted)?;
        output
            .ensure_len(Self::LATENCY_PROBE_FRAMES)
            .map_err(|_| ElasticError::PcmPoolBudgetExhausted)?;
        let frames = Self::LATENCY_PROBE_FRAMES
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?;
        for _ in 0..Self::LATENCY_PROBE_BLOCKS {
            probe.process(
                Some(source.as_ref()),
                output.as_mut(),
                Self::LATENCY_PROBE_FRAMES,
                frames,
                1.0,
            );
        }
        let frames = probe
            .latency()
            .ceil()
            .to_usize()
            .ok_or(ElasticError::SampleCountOverflow)?;
        Ok(ElasticLatency::new(frames, frames))
    }
}

impl fmt::Debug for BungeeElastic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BungeeElastic")
            .field("capabilities", &self.capabilities)
            .finish_non_exhaustive()
    }
}

impl ElasticEngine for BungeeElastic {
    fn prepare(config: ElasticConfig) -> Result<Self, ElasticError> {
        let latency = Self::latency(&config)?;
        let capabilities = ElasticCapabilities::new(config.shape(), latency)?;
        let source = Self::pooled(config.pool(), config.channels(), config.max_source_frames())?;
        let output = Self::pooled(config.pool(), config.channels(), config.max_output_frames())?;
        Ok(Self {
            stream: stream(
                config.sample_rate(),
                config.channels(),
                config.max_source_frames(),
            )?,
            capabilities,
            source,
            output,
            pitch: 1.0,
        })
    }

    fn capabilities(&self) -> ElasticCapabilities {
        self.capabilities
    }

    fn process(
        &mut self,
        request: ElasticRequest,
        source: &[f32],
        output: &mut [f32],
    ) -> Result<(), ElasticError> {
        self.capabilities
            .validate(request, source.len(), output.len())?;
        let channels = NonZeroUsize::new(self.capabilities.channels())
            .ok_or(ElasticError::InvalidChannelCount)?;
        let output_frames = request.output_frames();
        let requested = output_frames
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?;
        self.source
            .fill_interleaved(source, request.source_frames(), channels)
            .map_err(|_| ElasticError::PcmPoolBudgetExhausted)?;
        self.output
            .ensure_len(output_frames)
            .map_err(|_| ElasticError::PcmPoolBudgetExhausted)?;
        let rendered = self.stream.process(
            Some(self.source.as_ref()),
            self.output.as_mut(),
            request.source_frames(),
            requested,
            self.pitch,
        );
        if rendered != output_frames {
            return Err(ElasticError::EngineOutputFrameCount {
                actual: rendered,
                expected: output_frames,
            });
        }
        interleave_variable(self.output.as_ref(), 0..output_frames, output, channels);
        Ok(())
    }

    fn set_pitch(&mut self, scale: f64) -> Result<(), ElasticError> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err(ElasticError::InvalidPitch(scale));
        }
        self.pitch = scale;
        Ok(())
    }

    fn flush(&mut self, _output: &mut [f32]) -> Result<usize, ElasticError> {
        Ok(0)
    }

    fn reset(&mut self) -> Result<(), ElasticError> {
        self.stream = stream(
            self.capabilities.sample_rate(),
            self.capabilities.channels(),
            self.capabilities.max_source_frames(),
        )?;
        Ok(())
    }
}
