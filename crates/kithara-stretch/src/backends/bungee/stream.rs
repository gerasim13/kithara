use std::cmp::Ordering;

use bungee_sys::Request;
use num_traits::ToPrimitive;

use super::{
    buffer::{InputBuffer, PooledPlanar},
    ffi::{NativeOutput, NativeStretcher},
};
use crate::{ElasticConfig, ElasticError, ElasticRequest};

#[derive(fieldwork::Fieldwork)]
#[fieldwork(get, copy, vis = "pub(super)")]
pub(super) struct TerminalChunk {
    frames: usize,
    complete: bool,
}

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in)]
pub(super) struct StreamCore {
    input: InputBuffer,
    #[field(get, copy, vis = "pub(super)")]
    max_input_frames: usize,
    native: NativeStretcher,
    output: PooledPlanar,
    output_chunk: Option<NativeOutput>,
    output_consumed: usize,
    request: Request,
    request_pending: bool,
    samples_needed: f64,
}

impl StreamCore {
    const INPUT_LOOKAHEAD_DIVISOR: f64 = 2.0;
    const PIPELINE_GRAINS: usize = 4;
    const TERMINAL_GRAIN_LIMIT: usize = 64;

    pub(super) fn new(
        config: &ElasticConfig,
        max_source_frames: usize,
    ) -> Result<Self, ElasticError> {
        let native = NativeStretcher::new(config.sample_rate(), config.channels())?;
        let max_input_frames = native.max_input_frames()?;
        Ok(Self {
            input: InputBuffer::new(config, max_input_frames, max_source_frames)?,
            max_input_frames,
            native,
            output: PooledPlanar::new(config.pool(), config.channels(), max_input_frames)?,
            output_chunk: None,
            output_consumed: 0,
            request: Request {
                position: f64::NAN,
                speed: 1.0,
                pitch: 1.0,
                reset: 0,
            },
            request_pending: false,
            samples_needed: 0.0,
        })
    }

    pub(super) fn render(
        &mut self,
        source: Option<&[f32]>,
        request: ElasticRequest,
        pitch: f64,
        output: Option<&mut [f32]>,
    ) -> Result<(), ElasticError> {
        self.render_inner(source, request, pitch, output, false)
    }

    pub(super) fn probe_silence(&mut self, request: ElasticRequest) -> Result<(), ElasticError> {
        self.render_inner(None, request, 1.0, None, true)
    }

    fn render_inner(
        &mut self,
        source: Option<&[f32]>,
        request: ElasticRequest,
        pitch: f64,
        mut output: Option<&mut [f32]>,
        end_of_input: bool,
    ) -> Result<(), ElasticError> {
        let input_frames = request.source_frames();
        let output_frames = request.output_frames();
        self.input.append(source, input_frames)?;
        let input = input_frames
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?;
        let output_count = output_frames
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?;
        self.request.speed = input / output_count;
        self.request.pitch = pitch;
        self.samples_needed += output_count;
        let target = self
            .samples_needed
            .round()
            .to_usize()
            .ok_or(ElasticError::SampleCountOverflow)?;
        let mut rendered = 0;
        while rendered != target {
            rendered += self.consume(target - rendered, output.as_deref_mut(), rendered);
            if rendered == target {
                break;
            }
            let remaining = (target - rendered)
                .to_f64()
                .ok_or(ElasticError::SampleCountOverflow)?;
            let position = f64::from(self.input.end())
                - self
                    .max_input_frames
                    .to_f64()
                    .ok_or(ElasticError::SampleCountOverflow)?
                    / Self::INPUT_LOOKAHEAD_DIVISOR
                - input * remaining / output_count;
            self.request.reset =
                u8::from(position.partial_cmp(&self.request.position) != Some(Ordering::Greater));
            self.request.position = position;
            self.input.set_requested(self.native.specify(&self.request));
            self.request_pending = true;
            self.synthesise(true, end_of_input)?;
        }
        self.samples_needed -= rendered.to_f64().ok_or(ElasticError::SampleCountOverflow)?;
        Ok(())
    }

    fn synthesise(&mut self, valid: bool, end_of_input: bool) -> Result<(), ElasticError> {
        if !self.request_pending {
            return Err(ElasticError::EnginePreparation(
                "Bungee synthesis has no specified input grain",
            ));
        }
        self.input.analyse(&mut self.native, valid, end_of_input)?;
        self.output_chunk = Some(
            self.native
                .synthesise(&mut self.output.samples, self.output.stride)?,
        );
        self.output_consumed = 0;
        self.request_pending = false;
        Ok(())
    }

    fn consume(&mut self, wanted: usize, output: Option<&mut [f32]>, output_frame: usize) -> usize {
        let Some(chunk) = self.output_chunk.as_ref().filter(|chunk| chunk.valid) else {
            return 0;
        };
        let frames = wanted.min(chunk.frames.saturating_sub(self.output_consumed));
        if let Some(output) = output {
            self.output.copy_interleaved(
                self.output_consumed..self.output_consumed + frames,
                output,
                output_frame,
            );
        }
        self.output_consumed += frames;
        frames
    }

    pub(super) fn output_position(&self) -> Option<f64> {
        let chunk = self
            .output_chunk
            .as_ref()
            .filter(|chunk| chunk.valid && chunk.end.is_finite() && chunk.frames > 0)?;
        let consumed = self
            .output_consumed
            .to_f64()
            .zip(chunk.frames.to_f64())
            .map(|(consumed, frames)| consumed / frames)?;
        Some(chunk.begin + consumed * (chunk.end - chunk.begin))
    }

    fn consume_to_end(
        &mut self,
        source_end: f64,
        capacity: usize,
        output: Option<&mut [f32]>,
        output_frame: usize,
    ) -> Result<usize, ElasticError> {
        let Some(chunk) = self
            .output_chunk
            .as_ref()
            .filter(|chunk| chunk.valid && chunk.end.is_finite() && chunk.frames > 0)
        else {
            return Ok(0);
        };
        let until_end = if chunk.end <= chunk.begin {
            0
        } else {
            let chunk_frames = chunk
                .frames
                .to_f64()
                .ok_or(ElasticError::SampleCountOverflow)?;
            ((source_end - chunk.begin) * chunk_frames / (chunk.end - chunk.begin))
                .ceil()
                .clamp(0.0, chunk_frames)
                .to_usize()
                .ok_or(ElasticError::SampleCountOverflow)?
        };
        let available = until_end.saturating_sub(self.output_consumed);
        Ok(self.consume(available.min(capacity), output, output_frame))
    }

    pub(super) fn terminal_tail(
        &mut self,
        output: &mut [f32],
        capacity: usize,
    ) -> Result<TerminalChunk, ElasticError> {
        let source_end = f64::from(self.input.end());
        let mut drained = 0usize;
        let mut grains = 0;
        let mut stalled = 0;
        loop {
            drained = drained
                .checked_add(self.consume_to_end(
                    source_end,
                    capacity.saturating_sub(drained),
                    Some(output),
                    drained,
                )?)
                .ok_or(ElasticError::SampleCountOverflow)?;
            if self
                .output_position()
                .is_some_and(|position| position >= source_end)
            {
                if self.request_pending {
                    self.synthesise(true, true)?;
                }
                self.flush_invalid()?;
                self.clear();
                return Ok(TerminalChunk {
                    frames: drained,
                    complete: true,
                });
            }
            if drained == capacity {
                return Ok(TerminalChunk {
                    frames: drained,
                    complete: false,
                });
            }
            if grains == Self::TERMINAL_GRAIN_LIMIT {
                return Err(ElasticError::EnginePreparation(
                    "Bungee terminal output exceeded its fixed grain bound",
                ));
            }
            if !self.request_pending {
                self.native.next(&mut self.request);
                self.input.set_requested(self.native.specify(&self.request));
                self.request_pending = true;
            }
            let previous_end = self.output_chunk.as_ref().and_then(|chunk| {
                (chunk.valid && chunk.frames > 0 && chunk.end.is_finite()).then_some(chunk.end)
            });
            self.synthesise(true, true)?;
            if self.output_advances(previous_end) {
                stalled = 0;
            } else {
                stalled += 1;
            }
            if stalled > Self::PIPELINE_GRAINS {
                return Err(ElasticError::EnginePreparation(
                    "Bungee terminal output stopped advancing",
                ));
            }
            grains += 1;
        }
    }

    pub(super) fn discard(&mut self) -> Result<(), ElasticError> {
        if self.request_pending {
            self.synthesise(true, true)?;
        }
        self.flush_invalid()?;
        self.clear();
        Ok(())
    }

    fn flush_invalid(&mut self) -> Result<(), ElasticError> {
        for _ in 0..Self::PIPELINE_GRAINS {
            if self.native.is_flushed() {
                return Ok(());
            }
            self.request.position = f64::NAN;
            self.request.reset = 0;
            self.input.set_requested(self.native.specify(&self.request));
            self.request_pending = true;
            self.synthesise(false, true)?;
        }
        if self.native.is_flushed() {
            Ok(())
        } else {
            Err(ElasticError::EnginePreparation(
                "Bungee did not flush within its four-grain pipeline",
            ))
        }
    }

    pub(super) fn source_end(&self) -> i32 {
        self.input.end()
    }

    fn output_advances(&self, previous_end: Option<f64>) -> bool {
        self.output_chunk.as_ref().is_some_and(|chunk| {
            chunk.valid
                && chunk.frames > 0
                && chunk.end.is_finite()
                && chunk.end > chunk.begin
                && previous_end.is_none_or(|end| chunk.end > end)
        })
    }

    pub(super) fn clear(&mut self) {
        self.input.clear();
        self.output_chunk = None;
        self.output_consumed = 0;
        self.request.position = f64::NAN;
        self.request.speed = 1.0;
        self.request.reset = 0;
        self.request_pending = false;
        self.samples_needed = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use kithara_bufpool::PcmPool;
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn render_quantum_does_not_prefetch_the_next_control_grain() {
        const FRAMES: usize = 8192;

        let config = ElasticConfig::builder()
            .pool(PcmPool::default())
            .sample_rate(48_000)
            .channels(2)
            .max_source_frames(FRAMES)
            .max_output_frames(FRAMES)
            .build()
            .expect("the fixture shape is valid");
        let mut core = StreamCore::new(&config, FRAMES).expect("the fixture core prepares");
        let request = ElasticRequest::new(FRAMES, FRAMES).expect("unity request");
        let source = vec![0.0; FRAMES * config.channels()];
        let mut output = vec![0.0; source.len()];

        core.render(Some(&source), request, 1.0, Some(&mut output))
            .expect("the first quantum renders");

        assert!(
            !core.request_pending,
            "the next quantum must schedule its grain with its own rate and pitch"
        );
    }
}
