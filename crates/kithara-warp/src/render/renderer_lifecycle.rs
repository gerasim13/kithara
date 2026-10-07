use std::{mem, ops::ControlFlow};

use kithara_bufpool::{HasPool, SampleBuffer};
use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec, FrameCount, SampleCount};
use kithara_stretch::ElasticError;
use num_traits::ToPrimitive;
use tracing::warn;

use super::renderer::{PreparedQuantum, WarpRenderer};

impl<S> WarpRenderer<S>
where
    S: HasPool<f32>,
{
    fn advance_transition(
        &mut self,
        channels: usize,
        replacement: Option<SampleBuffer>,
    ) -> Option<AudioChunk> {
        if !self.active {
            return self.emit_pending_unity(replacement);
        }
        let complete = match self.drain_tail(channels) {
            Ok(complete) => complete,
            Err(error) => {
                warn!(%error, "time-stretch transition tail failed; preserving queued unity");
                self.retire_transition_tail(replacement);
                return None;
            }
        };
        if complete {
            self.finish_transition_tail();
        }
        let held_source_frames = if complete {
            0
        } else {
            self.held_source_frames()
        };
        if self
            .scratch
            .as_deref()
            .is_some_and(|scratch| !scratch.is_empty())
        {
            return self.emit(replacement, held_source_frames);
        }
        if complete {
            return self.emit_pending_unity(replacement);
        }

        warn!("time-stretch transition tail stopped without output");
        self.retire_transition_tail(replacement);
        None
    }

    fn begin_unity_transition(
        &mut self,
        meta: AudioChunkInfo,
        samples: &mut SampleBuffer,
        channels: usize,
    ) -> Result<(), ElasticError> {
        let tail_start_meta = self.last_input_meta;
        self.output_start_meta = None;
        if let Some(scratch) = self.scratch.as_mut() {
            scratch.clear();
        }
        let rounded = self
            .output_remainder
            .round()
            .max(0.0)
            .to_usize()
            .ok_or(ElasticError::SampleCountOverflow)?;
        if rounded > 1 {
            return Err(ElasticError::OutputFrameLimit {
                frames: rounded,
                limit: 1,
            });
        }
        self.render_terminal_pending(channels)?;
        if self.active && self.output_start_meta.is_none() {
            self.output_start_meta = tail_start_meta;
        }
        self.queue_unity(meta, samples)
    }

    fn drain_tail(&mut self, channels: usize) -> Result<bool, ElasticError> {
        if !self.active {
            return Ok(true);
        }
        let frame_limit = self
            .engine
            .as_ref()
            .ok_or(ElasticError::EnginePreparation("engine is unavailable"))?
            .capabilities()
            .latency()
            .output_frames();
        let sample_limit = frame_limit
            .checked_mul(channels)
            .ok_or(ElasticError::SampleCountOverflow)?;
        let scratch = self
            .scratch
            .as_mut()
            .ok_or(ElasticError::EnginePreparation(
                "output scratch is unavailable",
            ))?;
        let start = scratch.len();
        if start >= sample_limit && sample_limit > 0 {
            return Ok(false);
        }
        scratch
            .ensure_len(sample_limit)
            .map_err(|_| ElasticError::PoolCapacity)?;
        let drain = self
            .engine
            .as_mut()
            .ok_or(ElasticError::EnginePreparation("engine is unavailable"))?
            .flush(&mut scratch[start..sample_limit])?;
        let rendered_frames = FrameCount::new(drain.frames());
        let available_frames = (sample_limit - start) / channels;
        if rendered_frames.get() > available_frames {
            return Err(ElasticError::EngineOutputFrameCount {
                actual: rendered_frames.get(),
                expected: available_frames,
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

    /// Assemble an output chunk from `scratch`, preserving the exact source
    /// start and the latest decoder frontier. `replacement` is retained for
    /// shell-side preparation before the next checked tick.
    ///
    /// A non-empty output always carries the live source spec, since the default metadata sentinel
    /// has zero channels and cannot reach the resampler.
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

    fn emit_pending_unity(&mut self, replacement: Option<SampleBuffer>) -> Option<AudioChunk> {
        let meta = self.pending_unity_meta?;
        let replacement = replacement
            .or_else(|| self.scratch.take())
            .or_else(|| self.deferred_scratch.take());
        let Some(mut replacement) = replacement else {
            warn!("time-stretch queued unity has no reusable buffer");
            return None;
        };
        replacement.clear();
        let Some(samples) = self.pending_source.take() else {
            self.pending_source = Some(replacement);
            warn!("time-stretch queued unity buffer is unavailable");
            return None;
        };
        self.pending_source = Some(replacement);
        self.pending_unity_meta = None;
        self.pending_meta = None;
        self.last_input_meta = Some(meta);
        self.output_start_meta = None;
        self.record_rendered_source_end(meta, 0);
        Some(AudioChunk::new(meta, samples))
    }

    fn finish_transition_tail(&mut self) {
        self.reset_pending |= self.active;
        self.pending_meta = None;
        self.applied_pitch = f64::NAN;
        self.output_remainder = 0.0;
        self.source_frames_admitted = 0;
        self.primed_source_debt = 0;
        self.active = false;
        self.region = None;
    }

    /// Render the quantum's source frames from the residency at the engine's
    /// feed; the chunk has already extended the residency.
    fn process_active(
        &mut self,
        chunk: AudioChunk,
        speed: f32,
        prepared: Option<PreparedQuantum>,
    ) -> Option<AudioChunk> {
        if self.engine.is_none() || self.scratch.is_none() {
            warn!("time-stretch target was not prepared before rendering");
            self.defer_scratch(Some(chunk.samples));
            return None;
        }

        let AudioChunk { meta, samples } = chunk;
        if let Some(scratch) = self.scratch.as_mut() {
            scratch.clear();
        }

        let channels = usize::from(self.spec.channels.max(1));
        let source_end = meta
            .frame_offset
            .saturating_add(u64::try_from(samples.len() / channels).unwrap_or(u64::MAX));
        let feed = self.resident_feed.unwrap_or(meta.frame_offset);
        let frames = prepared.map_or_else(
            || usize::try_from(source_end.saturating_sub(feed)).unwrap_or(usize::MAX),
            |quantum| quantum.active_frames,
        );
        if frames > self.source_block_frames.get() {
            let error = ElasticError::SourceFrameLimit {
                frames,
                limit: self.source_block_frames.get(),
            };
            warn!(%error, "time-stretch rendering failed; dropping chunk");
            self.defer_scratch(Some(samples));
            return None;
        }
        let mut input = Self::meta_at_frame(meta, feed);
        input.frames = u32::try_from(frames).unwrap_or(u32::MAX);
        self.last_input_meta = Some(input);
        let landing = prepared.and_then(|quantum| quantum.landing_frames);
        let residency = self.residency.take();
        let rendered = residency
            .as_ref()
            .ok_or(ElasticError::PoolCapacity)
            .and_then(|resident| {
                let end = feed
                    .checked_add(
                        u64::try_from(frames).map_err(|_| ElasticError::SampleCountOverflow)?,
                    )
                    .ok_or(ElasticError::SampleCountOverflow)?;
                let range = i64::try_from(feed)
                    .map_err(|_| ElasticError::SampleCountOverflow)
                    .and_then(|feed| resident.range(feed, end, channels))?;
                self.render_active(
                    input,
                    &resident.samples[range],
                    speed,
                    channels,
                    frames,
                    landing,
                )
            });
        self.residency = residency;
        let rendered =
            rendered.and_then(
                |()| match (self.residency.as_mut(), self.scratch.as_mut()) {
                    (Some(resident), Some(output)) => resident.blend_replacement(output, channels),
                    _ => Ok(()),
                },
            );
        if let Err(error) = rendered {
            warn!(%error, "time-stretch rendering failed; dropping chunk");
            self.retire_engine();
            self.clear_render_state();
            self.defer_scratch(Some(samples));
            return None;
        }
        self.source_frames_admitted = self
            .source_frames_admitted
            .saturating_add(u64::try_from(frames).unwrap_or(u64::MAX));
        let next = feed.saturating_add(u64::try_from(frames).unwrap_or(u64::MAX));
        self.resident_feed = (next < source_end).then_some(next);
        let held_source_frames = self.held_source_frames();
        self.emit(Some(samples), held_source_frames)
    }

    fn process_unity(&mut self, chunk: AudioChunk) -> Option<AudioChunk> {
        let channels = usize::from(self.spec.channels.max(1));
        if !self.active && self.pending_frames(channels) == 0 {
            self.record_rendered_source_end(chunk.meta, 0);
            return Some(chunk);
        }

        let AudioChunk { meta, mut samples } = chunk;
        if let Err(error) = self.begin_unity_transition(meta, &mut samples, channels) {
            warn!(%error, "time-stretch transition to passthrough failed; dropping chunk");
            self.retire_engine();
            self.clear_render_state();
            self.defer_scratch(Some(samples));
            return None;
        }

        self.advance_transition(channels, Some(samples))
    }

    fn queue_unity(
        &mut self,
        meta: AudioChunkInfo,
        samples: &mut SampleBuffer,
    ) -> Result<(), ElasticError> {
        let pending = self
            .pending_source
            .as_mut()
            .ok_or(ElasticError::PoolCapacity)?;
        if !pending.is_empty() {
            return Err(ElasticError::EnginePreparation(
                "time-stretch pending source was not committed before unity",
            ));
        }
        mem::swap(pending, samples);
        self.pending_unity_meta = Some(meta);
        Ok(())
    }

    fn retire_transition_tail(&mut self, replacement: Option<SampleBuffer>) {
        self.retire_engine();
        if let Some(scratch) = self.scratch.as_mut() {
            scratch.clear();
        }
        self.defer_scratch(replacement);
        self.pending_meta = None;
        self.output_start_meta = None;
        self.applied_pitch = f64::NAN;
        self.output_remainder = 0.0;
        self.source_frames_admitted = 0;
        self.primed_source_debt = 0;
        self.reset_pending = false;
        self.active = false;
        self.region = None;
    }
}

impl<S> WarpRenderer<S>
where
    S: HasPool<f32>,
{
    fn finish_flush(
        &mut self,
        output: Option<AudioChunk>,
        complete: bool,
        snapshot: Option<crate::RenderSnapshot>,
    ) -> Option<AudioChunk> {
        let output = match output {
            Some(output) if output.frames() == 0 => {
                self.defer_scratch(Some(output.samples));
                None
            }
            output => output,
        };
        if let Some(output) = output.as_ref() {
            self.commit_render(snapshot, output);
        }
        if complete {
            self.backend_transition_pending = false;
            self.active = false;
            self.pending_meta = None;
            self.source_frames_admitted = 0;
            self.primed_source_debt = 0;
            self.reset_pending = true;
        }
        output
    }

    /// Drain one buffered output chunk after source EOF or a transition.
    pub fn flush(&mut self) -> Option<AudioChunk> {
        if !self.requires_staging() {
            return None;
        }
        let snapshot = self.context.load();
        if self.reprime_pending {
            self.retire_for_reprime();
            return None;
        }
        if let Some(scratch) = self.scratch.as_mut() {
            scratch.clear();
        } else {
            warn!("time-stretch output scratch was not serviced before flush");
            return None;
        }
        self.output_start_meta = None;
        let channels = usize::from(self.spec.channels.max(1));
        if self.pending_unity_meta.is_some() {
            let output = self.advance_transition(channels, None);
            if let Some(output) = output.as_ref() {
                self.commit_render(snapshot, output);
            }
            return output;
        }
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
        let output = self.emit(None, held_source_frames);
        self.finish_flush(output, complete, snapshot)
    }

    /// Prepare deferred renderer state for the current source format.
    pub fn prepare(&mut self, spec: AudioSpec) {
        self.service_target(spec);
    }

    /// Render one complete decoded source chunk.
    ///
    /// Returns the original input when it requires splitting into prepared
    /// quanta. The caller retains the unconsumed suffix between operations.
    pub fn render(&mut self, mut chunk: AudioChunk) -> ControlFlow<AudioChunk, Option<AudioChunk>> {
        if self.transition_pending() || self.engine_outdated() {
            return ControlFlow::Break(chunk);
        }
        if !self.requires_staging() && self.plan.is_some() {
            return ControlFlow::Break(chunk);
        }
        let snapshot = self.context.load();
        self.prepared_quantum = None;
        let rate = self.rate;
        let speed = match self.preview_speed(rate.speed(), chunk.frames().max(1)) {
            Ok(speed) => speed,
            Err(error) => {
                warn!(%error, "time-stretch speed smoothing failed");
                return ControlFlow::Break(chunk);
            }
        };
        chunk.meta.render_revision = rate.revision();
        ControlFlow::Continue(self.render_at(chunk, speed, snapshot, None, rate.speed()))
    }

    fn render_at(
        &mut self,
        chunk: AudioChunk,
        speed: f32,
        snapshot: Option<crate::RenderSnapshot>,
        prepared: Option<PreparedQuantum>,
        target_speed: f32,
    ) -> Option<AudioChunk> {
        if chunk.spec() != self.spec {
            warn!(
                expected = %self.spec,
                actual = %chunk.spec(),
                "time-stretch target was not serviced before a format change"
            );
            self.defer_scratch(Some(chunk.samples));
            return None;
        }
        if self.transition_pending() {
            warn!("time-stretch transition must drain before accepting new input");
            self.defer_scratch(Some(chunk.samples));
            return None;
        }

        let output = self.render_manual(chunk, speed, prepared);
        if let Some(output) = output.as_ref() {
            self.commit_rate_render(snapshot, output, speed, target_speed);
        }
        output
    }

    fn render_manual(
        &mut self,
        chunk: AudioChunk,
        speed: f32,
        prepared: Option<PreparedQuantum>,
    ) -> Option<AudioChunk> {
        if let Some(residency) = self.residency.as_mut()
            && let Err(error) = residency.retain_manual(
                chunk.meta,
                &chunk.samples,
                self.rendered_source_end.map(|(source, _)| source),
            )
        {
            warn!(%error, "source history retention failed");
            self.defer_scratch(Some(chunk.samples));
            return None;
        }
        if self.unity_passthrough(speed) {
            return self.process_unity(chunk);
        }
        if let Some(prepared) = prepared
            && let Err(error) = self.activate_prepared_quantum(&chunk, prepared)
        {
            warn!(%error, "time-stretch activation failed; dropping chunk");
            self.retire_engine();
            self.clear_render_state();
            self.defer_scratch(Some(chunk.samples));
            return None;
        }
        self.process_active(chunk, speed, prepared)
    }

    /// Render the source span selected by [`Self::prepare_quantum`].
    ///
    /// Returns the unchanged input if no matching quantum was prepared.
    pub fn render_quantum(
        &mut self,
        mut chunk: AudioChunk,
    ) -> ControlFlow<AudioChunk, Option<AudioChunk>> {
        let Some(prepared) = self.prepared_quantum else {
            return ControlFlow::Break(chunk);
        };
        if chunk.frames() != prepared.frames
            || chunk.meta.frame_offset != prepared.source_start
            || chunk.spec() != self.spec
            || self.transition_pending()
        {
            return ControlFlow::Break(chunk);
        }
        self.prepared_quantum = None;
        let snapshot = self.context.load();
        chunk.meta.render_revision = prepared.rate.revision();
        ControlFlow::Continue(self.render_at(
            chunk,
            prepared.speed,
            snapshot,
            Some(prepared),
            prepared.rate.speed(),
        ))
    }

    /// Discard renderer state after a source discontinuity.
    pub fn reset(&mut self) {
        self.reset_pending = true;
        self.clear_render_state();
        self.committed = None;
        self.snap_speed();
    }
}

impl<S: HasPool<f32>> WarpRenderer<S> {
    /// Retire the running engine's tail into the replacement the re-primed
    /// engine fades from, leaving the held source in the residency.
    fn retire_for_reprime(&mut self) {
        match self.retain_replacement() {
            Ok(()) => {
                if let Some(pending) = self.pending_source.as_mut() {
                    pending.clear();
                }
                self.pending_meta = None;
                self.output_remainder = 0.0;
                self.resident_feed = None;
            }
            Err(error) => {
                warn!(%error, "time-stretch re-prime retirement failed");
                self.retire_engine();
                self.clear_render_state();
            }
        }
    }

    /// Drain the running engine's tail into the replacement the next engine
    /// fades from. A fade still under way blends into that tail on the way, so
    /// the next fade starts from what sounds now.
    fn retain_replacement(&mut self) -> Result<(), ElasticError> {
        let channels = usize::from(self.spec.channels.max(1));
        let resident = self.residency.as_mut().ok_or(ElasticError::PoolCapacity)?;
        resident.next_replacement.clear();
        let capacity = resident.next_replacement.capacity() / channels;
        let quantum = self
            .engine
            .as_ref()
            .ok_or(ElasticError::EnginePreparation("engine is unavailable"))?
            .capabilities()
            .latency()
            .output_frames()
            .max(1);
        for _ in 0..=capacity.div_ceil(quantum) {
            self.scratch
                .as_mut()
                .ok_or(ElasticError::PoolCapacity)?
                .clear();
            let complete = self.drain_tail(channels)?;
            let resident = self.residency.as_mut().ok_or(ElasticError::PoolCapacity)?;
            let output = self.scratch.as_mut().ok_or(ElasticError::PoolCapacity)?;
            if resident.next_replacement.len() + output.len() > resident.next_replacement.capacity()
            {
                return Err(ElasticError::PoolCapacity);
            }
            resident.blend_replacement(output, channels)?;
            resident
                .next_replacement
                .try_extend_from_slice(output)
                .map_err(|_| ElasticError::PoolCapacity)?;
            output.clear();
            if complete {
                mem::swap(&mut resident.replacement, &mut resident.next_replacement);
                resident.next_replacement.clear();
                resident.replacement_offset = 0;
                self.backend_transition_pending = false;
                self.reprime_pending = false;
                self.active = false;
                self.applied_pitch = f64::NAN;
                self.reset_pending = false;
                return Ok(());
            }
        }
        Err(ElasticError::EnginePreparation(
            "backend drain exceeds its capability bound",
        ))
    }
}
