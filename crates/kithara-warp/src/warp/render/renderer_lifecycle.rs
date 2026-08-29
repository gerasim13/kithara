use kithara_bufpool::SampleBuffer;
use kithara_signal::{AudioChunk, AudioSpec, FrameCount, SampleCount};
use kithara_stretch::ElasticError;
use tracing::warn;

use super::renderer::WarpRenderer;

impl WarpRenderer {
    /// Assemble an output chunk from `scratch`, preserving the exact source
    /// start and the latest decoder frontier. `replacement` is retained for
    /// shell-side preparation before the next checked tick.
    fn emit(
        &mut self,
        replacement: Option<SampleBuffer>,
        held_source_frames: u64,
    ) -> Option<AudioChunk> {
        let total = self.scratch.as_deref().map_or(0, <[f32]>::len);
        if total == 0 {
            self.defer_scratch(replacement);
            return None;
        }
        let frames = match self.spec.frame_count(SampleCount::new(total)) {
            Ok(frames) => frames,
            Err(error) => {
                warn!(?error, total, "discarding malformed Warp output shape");
                self.scratch.take();
                self.defer_scratch(replacement);
                return None;
            }
        };
        let mut meta = self.last_input_meta.unwrap_or_default();
        self.record_rendered_source_end(meta, held_source_frames);
        // A non-empty output always carries the live source spec. The default
        // metadata sentinel has zero channels and cannot reach the resampler.
        meta.spec = self.spec;
        meta.frames = u32::try_from(frames.get()).unwrap_or(u32::MAX);
        if let Some(start) = self.output_start_meta.take() {
            if start.frame_offset != meta.frame_offset {
                meta.source_byte_offset = None;
                meta.source_bytes = 0;
            }
            meta.frame_offset = start.frame_offset;
            meta.timestamp = start.timestamp;
        }
        let samples = self.scratch.take()?;
        self.defer_scratch(replacement);
        Some(AudioChunk::new(meta, samples))
    }

    fn drain_tail(&mut self, channels: usize) -> Result<bool, ElasticError> {
        if !self.active {
            return Ok(true);
        }
        let tail_frames = self
            .engine
            .as_ref()
            .ok_or(ElasticError::EnginePreparation("engine is unavailable"))?
            .capabilities()
            .terminal_chunk_frames();
        let tail_samples = SampleCount::new(
            tail_frames
                .checked_mul(channels)
                .ok_or(ElasticError::SampleCountOverflow)?,
        );
        let scratch = self
            .scratch
            .as_mut()
            .ok_or(ElasticError::EnginePreparation(
                "output scratch is unavailable",
            ))?;
        let start = scratch.len();
        let end = start
            .checked_add(tail_samples.get())
            .ok_or(ElasticError::SampleCountOverflow)?;
        if end > scratch.capacity() {
            return Err(ElasticError::OutputFrameLimit {
                frames: end / channels,
                limit: scratch.capacity() / channels,
            });
        }
        scratch
            .ensure_len(end)
            .map_err(|_| ElasticError::SamplePoolBudgetExhausted)?;
        let drain = self
            .engine
            .as_mut()
            .ok_or(ElasticError::EnginePreparation("engine is unavailable"))?
            .flush(&mut scratch[start..end])?;
        let rendered_frames = FrameCount::new(drain.frames());
        if rendered_frames.get() > tail_frames {
            return Err(ElasticError::EngineOutputFrameCount {
                actual: rendered_frames.get(),
                expected: tail_frames,
            });
        }
        let rendered_samples = rendered_frames
            .get()
            .checked_mul(channels)
            .map(SampleCount::new)
            .ok_or(ElasticError::SampleCountOverflow)?;
        scratch.truncate(start + rendered_samples.get());
        if !drain.complete() && rendered_frames.get() == 0 {
            return Err(ElasticError::EnginePreparation(
                "time-stretch terminal drain stopped advancing",
            ));
        }
        Ok(drain.complete())
    }

    fn process_active(&mut self, chunk: AudioChunk, speed: f32) -> Option<AudioChunk> {
        if self.engine.is_none() || self.scratch.is_none() {
            warn!("time-stretch target was not prepared before rendering");
            self.defer_scratch(Some(chunk.samples));
            return None;
        }

        let AudioChunk { meta, samples } = chunk;
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
        if let Err(error) = self.render_active(meta, &samples, speed, channels, frames) {
            warn!(%error, "time-stretch rendering failed; dropping chunk");
            self.retire_engine();
            self.clear_render_state();
            self.defer_scratch(Some(samples));
            return None;
        }
        self.source_frames_admitted = self
            .source_frames_admitted
            .saturating_add(u64::try_from(frames).unwrap_or(u64::MAX));
        let held_source_frames = self.held_source_frames();
        self.emit(Some(samples), held_source_frames)
    }

    #[doc(hidden)]
    pub fn prepare(&mut self, spec: AudioSpec) {
        self.service_target(spec);
    }

    #[doc(hidden)]
    pub fn flush(&mut self) -> Option<AudioChunk> {
        if let Some(scratch) = self.scratch.as_mut() {
            scratch.clear();
        } else {
            warn!("time-stretch output scratch was not serviced before flush");
            return None;
        }
        self.output_start_meta = None;
        let channels = usize::from(self.spec.channels.max(1));
        let result = self
            .render_terminal_pending(channels)
            .and_then(|()| self.drain_tail(channels));
        let complete = match result {
            Ok(complete) => complete,
            Err(error) => {
                warn!(%error, "time-stretch engine flush failed");
                self.retire_engine();
                self.clear_render_state();
                return None;
            }
        };
        let held_source_frames = if complete {
            0
        } else {
            self.held_source_frames()
        };
        self.emit(None, held_source_frames)
    }

    fn render_at_speed(&mut self, chunk: AudioChunk, speed: f32) -> Option<AudioChunk> {
        if chunk.spec() != self.spec {
            warn!(
                expected = %self.spec,
                actual = %chunk.spec(),
                "time-stretch target was not serviced before a format change"
            );
            self.defer_scratch(Some(chunk.samples));
            return None;
        }
        if self.can_passthrough(speed) {
            self.record_rendered_source_end(chunk.meta, 0);
            return Some(chunk);
        }
        self.process_active(chunk, speed)
    }

    #[doc(hidden)]
    pub fn render(&mut self, chunk: AudioChunk) -> Option<AudioChunk> {
        self.prepared_quantum_speed = None;
        let speed = self.controls.speed();
        self.render_at_speed(chunk, speed)
    }

    /// Render the source quantum paired with the speed sampled by
    /// [`Self::prepare_quantum`].
    #[doc(hidden)]
    pub fn render_quantum(&mut self, chunk: AudioChunk) -> Option<AudioChunk> {
        let Some(speed) = self.prepared_quantum_speed.take() else {
            warn!("time-stretch quantum was not prepared before rendering");
            self.defer_scratch(Some(chunk.samples));
            return None;
        };
        self.render_at_speed(chunk, speed)
    }

    #[doc(hidden)]
    pub fn reset(&mut self) {
        self.reset_pending = true;
        self.clear_render_state();
    }
}
