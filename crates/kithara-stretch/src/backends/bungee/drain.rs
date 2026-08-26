use num_traits::ToPrimitive;

use super::{StreamCore, TerminalChunk};
use crate::ElasticError;

impl StreamCore {
    pub(in super::super) fn output_position(&self) -> Option<f64> {
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

    pub(in super::super) fn terminal_tail(
        &mut self,
        output: &mut [f32],
        capacity: usize,
    ) -> Result<TerminalChunk, ElasticError> {
        let source_end = f64::from(self.input.end());
        let mut drained = 0usize;
        let mut grains = 0;
        let mut stalled = 0;
        loop {
            if let Some(anchor) = self.anchor {
                self.discard_before(anchor)?;
            }
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
            let previous_end = self.output_chunk.as_ref().and_then(|chunk| {
                (chunk.valid && chunk.frames > 0 && chunk.end.is_finite()).then_some(chunk.end)
            });
            if self.anchor.is_some() {
                self.schedule_anchored(true)?;
            } else {
                if !self.request_pending {
                    self.native.next(&mut self.request);
                    self.input.set_requested(self.native.specify(&self.request));
                    self.request_pending = true;
                }
                self.synthesise(true, true)?;
            }
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

    pub(in super::super) fn discard(&mut self) -> Result<(), ElasticError> {
        if self.request_pending {
            self.synthesise(true, true)?;
        }
        self.flush_invalid()?;
        self.clear();
        Ok(())
    }

    pub(super) fn flush_invalid(&mut self) -> Result<(), ElasticError> {
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

    pub(super) fn recover(&mut self) -> Result<(), ElasticError> {
        let result = self
            .finish_pending_invalid()
            .and_then(|()| self.flush_invalid());
        self.clear();
        result
    }

    fn finish_pending_invalid(&mut self) -> Result<(), ElasticError> {
        if !self.request_pending {
            return Ok(());
        }
        self.input.analyse(&mut self.native, false, true)?;
        self.request_pending = false;
        self.native
            .synthesise(&mut self.output.samples, self.output.stride)?;
        Ok(())
    }

    pub(in super::super) fn source_end(&self) -> i32 {
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
        self.anchor = None;
        self.cue_grain_pending = false;
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
