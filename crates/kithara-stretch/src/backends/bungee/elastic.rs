use std::fmt;

use num_traits::ToPrimitive;

use super::stream::StreamCore;
use crate::{
    ElasticCapabilities, ElasticConfig, ElasticEngine, ElasticError, ElasticLatency,
    ElasticRequest, elastic::PitchRange,
};

/// Exact-span Bungee engine.
#[non_exhaustive]
pub struct BungeeElastic {
    capabilities: ElasticCapabilities,
    core: StreamCore,
    pitch: f64,
    tail_armed: bool,
}

impl BungeeElastic {
    const LATENCY_PROBE_BLOCKS: usize = 4;

    fn latency(
        core: &mut StreamCore,
        config: &ElasticConfig,
    ) -> Result<ElasticLatency, ElasticError> {
        let probe_frames = config.max_source_frames().min(config.max_output_frames());
        let request = ElasticRequest::new(probe_frames, probe_frames)?;
        for _ in 0..Self::LATENCY_PROBE_BLOCKS {
            core.probe_silence(request)?;
        }
        let output_position = core
            .output_position()
            .ok_or(ElasticError::EnginePreparation(
                "Bungee latency probe produced no timed output",
            ))?;
        let frames = (f64::from(core.source_end()) - output_position)
            .ceil()
            .to_usize()
            .ok_or(ElasticError::SampleCountOverflow)?;
        core.discard()?;
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
        let mut core = StreamCore::new(&config, config.max_source_frames())?;
        let latency = Self::latency(&mut core, &config)?;
        let terminal_chunk_frames = core.max_input_frames();
        let capabilities =
            ElasticCapabilities::new(config.shape(), latency, terminal_chunk_frames)?;
        Ok(Self {
            core,
            capabilities,
            pitch: 1.0,
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
        self.core
            .render(Some(source), request, self.pitch, Some(output))?;
        self.tail_armed = true;
        Ok(())
    }

    fn set_pitch(&mut self, scale: f64) -> Result<(), ElasticError> {
        self.pitch = PitchRange::validate(scale)?;
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
        let chunk = self.core.terminal_tail(output, tail_frames)?;
        self.tail_armed = !chunk.complete();
        Ok(chunk.frames())
    }

    fn reset(&mut self) -> Result<(), ElasticError> {
        self.core.discard()?;
        self.tail_armed = false;
        Ok(())
    }
}
