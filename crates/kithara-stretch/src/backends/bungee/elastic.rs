use std::fmt;

use num_traits::ToPrimitive;

use super::stream::StreamCore;
use crate::{
    ElasticCapabilities, ElasticConfig, ElasticDrain, ElasticEngine, ElasticError, ElasticLatency,
    ElasticRequest, elastic::PitchScale,
};

/// Exact-span Bungee engine.
pub(crate) struct BungeeElastic {
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
        let source_frames = core.source_latency_frames()?;
        let output_position = core
            .output_position()
            .ok_or(ElasticError::EnginePreparation(
                "Bungee latency probe produced no timed output",
            ))?;
        let total_latency = f64::from(core.source_end()) - output_position;
        let total_frames = total_latency
            .ceil()
            .to_usize()
            .ok_or(ElasticError::SampleCountOverflow)?;
        let output_frames = total_frames
            .checked_sub(source_frames)
            .filter(|frames| *frames > 0)
            .ok_or(ElasticError::EnginePreparation(
                "Bungee latency probe produced no output-side latency",
            ))?;
        core.set_source_latency_frames(source_frames)?;
        core.discard()?;
        Ok(ElasticLatency::new(source_frames, output_frames))
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
        let maximum_warm_source = config.rate_envelope().max_source_frames_per_output()
            * latency
                .output_frames()
                .to_f64()
                .ok_or(ElasticError::SampleCountOverflow)?;
        let maximum_warm_source = maximum_warm_source
            .ceil()
            .to_usize()
            .ok_or(ElasticError::SampleCountOverflow)?;
        let prime_context = latency
            .source_frames()
            .checked_add(latency.source_frames())
            .and_then(|frames| frames.checked_add(maximum_warm_source))
            .ok_or(ElasticError::SampleCountOverflow)?;
        let retained = core.max_input_frames().max(prime_context);
        let input_capacity = config
            .max_source_frames()
            .checked_add(retained)
            .ok_or(ElasticError::SampleCountOverflow)?;
        core.prepare_input_capacity(input_capacity)?;
        let terminal_chunk_frames = core.max_input_frames();
        let capabilities = ElasticCapabilities::new(config.shape(), latency, terminal_chunk_frames);
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

    fn prime(
        &mut self,
        request: ElasticRequest,
        source_history: &[f32],
        source_lookahead: &[f32],
        source: &[f32],
        discarded_output: &mut [f32],
    ) -> Result<(), ElasticError> {
        self.capabilities.validate_prime(
            request,
            source_history.len(),
            source_lookahead.len(),
            source.len(),
            discarded_output.len(),
        )?;
        self.tail_armed = false;
        self.core.prime(
            source_history,
            source_lookahead,
            request,
            source,
            self.pitch,
            discarded_output,
        )?;
        self.tail_armed = true;
        Ok(())
    }

    #[cfg_attr(feature = "perf", hotpath::measure)]
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
        self.pitch =
            f64::from(PitchScale::checked(scale).ok_or(ElasticError::InvalidPitch(scale))?);
        Ok(())
    }

    fn flush(&mut self, output: &mut [f32]) -> Result<ElasticDrain, ElasticError> {
        if !self.tail_armed {
            return Ok(ElasticDrain::new(0, true));
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
        Ok(chunk)
    }

    fn reset(&mut self) -> Result<(), ElasticError> {
        self.core.discard()?;
        self.tail_armed = false;
        Ok(())
    }
}
