use std::fmt;

use num_traits::ToPrimitive;
use signalsmith_stretch::Stretch;

use crate::{
    ElasticCapabilities, ElasticConfig, ElasticEngine, ElasticError, ElasticLatency,
    ElasticPriming, ElasticRequest, elastic::PitchRange,
};

const CHANNEL_COUNT_LIMIT: u32 = u32::MAX;

fn engine(sample_rate: u32, channels: usize) -> (Stretch, ElasticLatency) {
    let inner = Stretch::preset_default(
        u32::try_from(channels).unwrap_or(CHANNEL_COUNT_LIMIT),
        sample_rate,
    );
    let latency = ElasticLatency::new(inner.input_latency(), inner.output_latency());
    (inner, latency)
}

/// Exact-span Signalsmith engine, prepared for fixed maximum source and output
/// blocks.
#[non_exhaustive]
pub struct SignalsmithElastic {
    inner: Stretch,
    capabilities: ElasticCapabilities,
    tail_armed: bool,
}

impl fmt::Debug for SignalsmithElastic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignalsmithElastic")
            .field("capabilities", &self.capabilities)
            .finish_non_exhaustive()
    }
}

impl ElasticEngine for SignalsmithElastic {
    fn prepare(config: ElasticConfig) -> Result<Self, ElasticError> {
        u32::try_from(config.channels())
            .map_err(|_| ElasticError::ChannelCountOutOfRange(config.channels()))?;
        let (mut inner, latency) = engine(config.sample_rate(), config.channels());
        inner.set_transpose_factor(1.0, None);
        Ok(Self {
            inner,
            capabilities: ElasticCapabilities::new(
                config.shape(),
                latency,
                latency.output_frames(),
            )?,
            tail_armed: false,
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
        self.inner.process(source, output);
        self.tail_armed = true;
        Ok(())
    }

    fn set_pitch(&mut self, scale: f64) -> Result<(), ElasticError> {
        let factor = PitchRange::validate(scale)?
            .to_f32()
            .ok_or(ElasticError::InvalidPitch(scale))?;
        self.inner.set_transpose_factor(factor, None);
        Ok(())
    }

    fn flush(&mut self, output: &mut [f32]) -> Result<usize, ElasticError> {
        if !self.tail_armed {
            return Ok(0);
        }
        let tail_frames = self.capabilities.terminal_chunk_frames();
        let expected = self.capabilities.samples(tail_frames)?;
        if output.len() != expected {
            return Err(ElasticError::OutputSampleCount {
                actual: output.len(),
                expected,
            });
        }
        self.inner.flush(output);
        self.tail_armed = false;
        Ok(tail_frames)
    }

    fn reset(&mut self) -> Result<(), ElasticError> {
        self.inner.reset();
        self.tail_armed = false;
        Ok(())
    }
}

impl ElasticPriming for SignalsmithElastic {
    fn prime(
        &mut self,
        request: ElasticRequest,
        source_history: &[f32],
        source: &[f32],
        discarded_output: &mut [f32],
    ) -> Result<(), ElasticError> {
        let latency = self.capabilities.latency();
        if request.output_frames() != latency.output_frames() {
            return Err(ElasticError::WarmupOutputFrameCount {
                actual: request.output_frames(),
                expected: latency.output_frames(),
            });
        }
        let expected_history_samples = self.capabilities.samples(latency.source_frames())?;
        if source_history.len() != expected_history_samples {
            return Err(ElasticError::HistorySampleCount {
                actual: source_history.len(),
                expected: expected_history_samples,
            });
        }
        self.capabilities
            .validate_spans(request, source.len(), discarded_output.len())?;
        let playback_rate = request.source_frames_per_output()?;
        self.inner.reset();
        self.inner.seek(source_history, playback_rate);
        self.inner.process(source, discarded_output);
        self.tail_armed = true;
        Ok(())
    }
}
