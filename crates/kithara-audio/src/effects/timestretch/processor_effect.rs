use kithara_bufpool::PcmBuf;
use kithara_decode::{PcmChunk, PcmSpec};
use kithara_stretch::ElasticError;
use num_traits::ToPrimitive;
use tracing::warn;

use super::processor::TimeStretchProcessor;
use crate::traits::AudioEffect;

impl TimeStretchProcessor {
    /// Assemble an output chunk from `scratch`, preserving the exact source
    /// start and the latest decoder frontier. `replacement` is retained for
    /// shell-side preparation before the next checked tick.
    fn emit(&mut self, replacement: Option<PcmBuf>) -> Option<PcmChunk> {
        let total = self.scratch.as_deref().map_or(0, <[f32]>::len);
        if total == 0 {
            self.defer_scratch(replacement);
            return None;
        }
        let channels = usize::from(self.spec.channels.max(1));
        let mut meta = self.last_input_meta.unwrap_or_default();
        // A non-empty output always carries the live source spec. The default
        // metadata sentinel has zero channels and cannot reach the resampler.
        meta.spec = self.spec;
        meta.frames = u32::try_from(total / channels).unwrap_or(u32::MAX);
        if let Some(start) = self.output_start_meta.take() {
            if start.frame_offset != meta.frame_offset {
                meta.source_byte_offset = None;
                meta.source_bytes = 0;
            }
            meta.frame_offset = start.frame_offset;
            meta.timestamp = start.timestamp;
        }
        let pcm = self.scratch.take()?;
        self.defer_scratch(replacement);
        Some(PcmChunk::new(meta, pcm))
    }

    fn reset_for_passthrough(&mut self) {
        let has_pending = self
            .pending_source
            .as_deref()
            .is_some_and(|source| !source.is_empty());
        if !self.active && !has_pending {
            return;
        }
        self.reset_pending = self.active;
        self.clear_pending_source();
        self.applied_pitch = f64::NAN;
        self.output_remainder = 0.0;
        self.active = false;
    }

    fn process_unity(&mut self, chunk: PcmChunk) -> Option<PcmChunk> {
        let channels = usize::from(self.spec.channels.max(1));
        if self.pending_frames(channels) == 0 {
            self.reset_for_passthrough();
            return Some(chunk);
        }

        let PcmChunk { meta, samples } = chunk;
        self.last_input_meta = Some(meta);
        self.output_start_meta = None;
        if let Some(scratch) = self.scratch.as_mut() {
            scratch.clear();
        }
        let rounded = self.output_remainder.round().max(0.0).to_usize();
        match rounded {
            Some(0) => {
                self.reset_for_passthrough();
                Some(PcmChunk::new(meta, samples))
            }
            Some(1) => {
                let result = self
                    .render_terminal_pending(channels)
                    .and_then(|()| self.append_scratch(&samples, channels));
                if let Err(error) = result {
                    warn!(%error, "time-stretch transition to passthrough failed; dropping chunk");
                    self.retire_engine();
                    self.clear_render_state();
                    self.defer_scratch(Some(samples));
                    return None;
                }
                self.reset_for_passthrough();
                self.emit(Some(samples))
            }
            Some(frames) => {
                warn!(
                    frames,
                    "time-stretch pending span rounded outside one-frame bound"
                );
                self.retire_engine();
                self.clear_render_state();
                self.defer_scratch(Some(samples));
                None
            }
            None => {
                warn!("time-stretch pending span could not be represented");
                self.retire_engine();
                self.clear_render_state();
                self.defer_scratch(Some(samples));
                None
            }
        }
    }
}

impl AudioEffect for TimeStretchProcessor {
    fn service_deferred(&mut self, spec: PcmSpec) {
        self.service_target(spec);
    }

    fn flush(&mut self) -> Option<PcmChunk> {
        if let Some(scratch) = self.scratch.as_mut() {
            scratch.clear();
        } else {
            warn!("time-stretch output scratch was not serviced before flush");
            return None;
        }
        self.output_start_meta = None;
        let channels = usize::from(self.spec.channels.max(1));
        let result = self.render_terminal_pending(channels).and_then(|()| {
            if self.active {
                let tail_frames = self
                    .engine
                    .as_ref()
                    .ok_or(ElasticError::EnginePreparation("engine is unavailable"))?
                    .capabilities()
                    .terminal_chunk_frames();
                let tail_samples = tail_frames
                    .checked_mul(channels)
                    .ok_or(ElasticError::SampleCountOverflow)?;
                let scratch = self
                    .scratch
                    .as_mut()
                    .ok_or(ElasticError::EnginePreparation(
                        "output scratch is unavailable",
                    ))?;
                let start = scratch.len();
                let end = start
                    .checked_add(tail_samples)
                    .ok_or(ElasticError::SampleCountOverflow)?;
                if end > scratch.capacity() {
                    return Err(ElasticError::OutputFrameLimit {
                        frames: end / channels,
                        limit: scratch.capacity() / channels,
                    });
                }
                scratch
                    .ensure_len(end)
                    .map_err(|_| ElasticError::PcmPoolBudgetExhausted)?;
                let rendered_frames = self
                    .engine
                    .as_mut()
                    .ok_or(ElasticError::EnginePreparation("engine is unavailable"))?
                    .flush(&mut scratch[start..end])?;
                if rendered_frames > tail_frames {
                    return Err(ElasticError::EngineOutputFrameCount {
                        actual: rendered_frames,
                        expected: tail_frames,
                    });
                }
                let rendered_samples = rendered_frames
                    .checked_mul(channels)
                    .ok_or(ElasticError::SampleCountOverflow)?;
                scratch.truncate(start + rendered_samples);
            }
            Ok(())
        });
        if let Err(error) = result {
            warn!(%error, "time-stretch engine flush failed");
            self.retire_engine();
            self.clear_render_state();
            return None;
        }
        self.emit(None)
    }

    fn process(&mut self, chunk: PcmChunk) -> Option<PcmChunk> {
        if chunk.spec() != self.spec {
            warn!(
                expected = %self.spec,
                actual = %chunk.spec(),
                "time-stretch target was not serviced before a format change"
            );
            self.defer_scratch(Some(chunk.samples));
            return None;
        }

        let speed = self.controls.speed().max(Self::MIN_SPEED);
        if self.unity_passthrough(speed) {
            return self.process_unity(chunk);
        }
        if self.engine.is_none() || self.scratch.is_none() {
            warn!("time-stretch target was not prepared before rendering");
            self.defer_scratch(Some(chunk.samples));
            return None;
        }

        let PcmChunk { meta, samples } = chunk;
        self.last_input_meta = Some(meta);
        self.output_start_meta = None;
        if let Some(scratch) = self.scratch.as_mut() {
            scratch.clear();
        }

        let channels = usize::from(self.spec.channels.max(1));
        let frames = samples.len() / channels;
        if frames > Self::MAX_SOURCE_FRAMES {
            let error = ElasticError::SourceFrameLimit {
                frames,
                limit: Self::MAX_SOURCE_FRAMES,
            };
            warn!(%error, "time-stretch rendering failed; dropping chunk");
            self.defer_scratch(Some(samples));
            return None;
        }
        let rendered = self.render_active(meta, &samples, speed, channels, frames);
        if let Err(error) = rendered {
            warn!(%error, "time-stretch rendering failed; dropping chunk");
            self.retire_engine();
            self.clear_render_state();
            self.defer_scratch(Some(samples));
            return None;
        }
        self.emit(Some(samples))
    }

    fn reset(&mut self) {
        self.reset_pending = true;
        self.clear_render_state();
    }
}
