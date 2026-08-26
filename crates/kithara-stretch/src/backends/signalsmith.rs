use std::fmt;

use kithara_bufpool::PcmBuf;
use num_traits::ToPrimitive;
use signalsmith_stretch::Stretch;

use crate::{
    ElasticCapabilities, ElasticConfig, ElasticEngine, ElasticError, ElasticLatency,
    ElasticRequest, elastic::PitchScale,
};

fn engine(sample_rate: u32, channels: u32) -> (Stretch, ElasticLatency) {
    let inner = Stretch::preset_default(channels, sample_rate);
    let native_input_latency = inner.input_latency();
    let native_output_latency = inner.output_latency();
    (
        inner,
        ElasticLatency::new(native_input_latency, native_output_latency),
    )
}

#[derive(Clone, Copy)]
enum TerminalState {
    Idle,
    Armed { rate: f64 },
    Flush,
}

/// Exact-span Signalsmith engine, prepared for fixed maximum source and output
/// blocks.
pub(crate) struct SignalsmithElastic {
    inner: Stretch,
    capabilities: ElasticCapabilities,
    prime_input: PcmBuf,
    terminal: TerminalState,
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
        let channels = u32::try_from(config.channels())
            .map_err(|_| ElasticError::ChannelCountOutOfRange(config.channels()))?;
        let (mut inner, latency) = engine(config.sample_rate(), channels);
        inner.set_transpose_factor(1.0, None);
        let minimum_rate = config.rate_envelope().min_source_frames_per_output();
        let terminal_process_frames = latency
            .source_frames()
            .to_f64()
            .map(|frames| (frames / minimum_rate).ceil())
            .and_then(|frames| frames.to_usize())
            .ok_or(ElasticError::SampleCountOverflow)?;
        let terminal_chunk_frames = terminal_process_frames.max(latency.output_frames());
        let capabilities = ElasticCapabilities::new(config.shape(), latency, terminal_chunk_frames);
        let prime_window_samples = capabilities.samples(latency.source_frames())?;
        let prime_samples = prime_window_samples
            .checked_add(prime_window_samples)
            .ok_or(ElasticError::SampleCountOverflow)?;
        let mut prime_input = config.pool().get();
        prime_input
            .ensure_len(prime_samples)
            .map_err(|_| ElasticError::PcmPoolBudgetExhausted)?;
        Ok(Self {
            inner,
            capabilities,
            prime_input,
            terminal: TerminalState::Idle,
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
        let playback_rate = request.source_frames_per_output()?;
        let history_end = source_history.len();
        let input_end = history_end
            .checked_add(source_lookahead.len())
            .ok_or(ElasticError::SampleCountOverflow)?;
        self.prime_input[..history_end].copy_from_slice(source_history);
        self.prime_input[history_end..input_end].copy_from_slice(source_lookahead);
        self.inner.reset();
        self.inner
            .seek(&self.prime_input[..input_end], playback_rate);
        self.inner.process(source, discarded_output);
        self.terminal = TerminalState::Armed {
            rate: playback_rate,
        };
        Ok(())
    }

    fn process(
        &mut self,
        request: ElasticRequest,
        source: &[f32],
        output: &mut [f32],
    ) -> Result<(), ElasticError> {
        self.capabilities
            .validate(request, source.len(), output.len())?;
        let rate = request.source_frames_per_output()?;
        self.inner.process(source, output);
        self.terminal = TerminalState::Armed { rate };
        Ok(())
    }

    fn set_pitch(&mut self, scale: f64) -> Result<(), ElasticError> {
        let factor = PitchScale::checked(scale)
            .map(f64::from)
            .ok_or(ElasticError::InvalidPitch(scale))?
            .to_f32()
            .ok_or(ElasticError::InvalidPitch(scale))?;
        self.inner.set_transpose_factor(factor, None);
        Ok(())
    }

    fn flush(&mut self, output: &mut [f32]) -> Result<usize, ElasticError> {
        if matches!(self.terminal, TerminalState::Idle) {
            return Ok(0);
        }
        let chunk_frames = self.capabilities.terminal_chunk_frames();
        let expected = self.capabilities.samples(chunk_frames)?;
        if output.len() != expected {
            return Err(ElasticError::OutputSampleCount {
                actual: output.len(),
                expected,
            });
        }
        match self.terminal {
            TerminalState::Armed { rate } => {
                let source_frames = self.capabilities.latency().source_frames();
                let output_frames = source_frames
                    .to_f64()
                    .map(|frames| (frames / rate).ceil())
                    .and_then(|frames| frames.to_usize())
                    .ok_or(ElasticError::SampleCountOverflow)?;
                let source_samples = self.capabilities.samples(source_frames)?;
                let output_samples = self.capabilities.samples(output_frames)?;
                self.prime_input[..source_samples].fill(0.0);
                self.inner.process(
                    &self.prime_input[..source_samples],
                    &mut output[..output_samples],
                );
                self.terminal = TerminalState::Flush;
                Ok(output_frames)
            }
            TerminalState::Flush => {
                let output_frames = self.capabilities.latency().output_frames();
                let output_samples = self.capabilities.samples(output_frames)?;
                self.inner.flush(&mut output[..output_samples]);
                self.terminal = TerminalState::Idle;
                Ok(output_frames)
            }
            TerminalState::Idle => Ok(0),
        }
    }

    fn reset(&mut self) -> Result<(), ElasticError> {
        self.inner.reset();
        self.terminal = TerminalState::Idle;
        Ok(())
    }
}
