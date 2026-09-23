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

    fn process_active(&mut self, chunk: AudioChunk, speed: f32) -> Option<AudioChunk> {
        if self.engine.is_none() || self.scratch.is_none() {
            warn!("time-stretch target was not prepared before rendering");
            self.defer_scratch(Some(chunk.samples));
            return None;
        }

        let AudioChunk { meta, samples } = chunk;
        self.last_input_meta = Some(meta);
        if let Some(scratch) = self.scratch.as_mut() {
            scratch.clear();
        }

        let channels = usize::from(self.spec.channels.max(1));
        let frames = samples.len() / channels;
        if frames > self.source_block_frames.get() {
            let error = ElasticError::SourceFrameLimit {
                frames,
                limit: self.source_block_frames.get(),
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
    /// Drain one buffered output chunk after source EOF or a transition.
    pub fn flush(&mut self) -> Option<AudioChunk> {
        let snapshot = self.context.load();
        if self.backend_transition_pending
            && self.projection.selected.is_some()
            && (self.projection.active.is_some() || self.projection.prepared.is_some())
        {
            if let Err(error) = self.retain_projected_replacement() {
                warn!(%error, "projected backend retirement failed");
            }
            return None;
        }
        if self
            .residency
            .as_ref()
            .is_some_and(|resident| resident.prepared.is_some())
        {
            return self.flush_resident_request(snapshot);
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

    fn finish_flush(
        &mut self,
        mut output: Option<AudioChunk>,
        complete: bool,
        snapshot: Option<crate::RenderSnapshot>,
    ) -> Option<AudioChunk> {
        let rejected = output.as_mut().and_then(|chunk| {
            let projection = self.projected_tail_cursor(chunk.frames())?;
            self.trim_projected_eof(chunk, projection).err()
        });
        if let Some(error) = rejected {
            warn!(%error, "projected EOF identity is uncovered");
            self.defer_scratch(output.map(|chunk| chunk.samples));
            return None;
        }
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

    /// Prepare deferred renderer state for the current source format.
    pub fn prepare(&mut self, spec: AudioSpec) {
        self.service_target(spec);
    }

    /// Render one complete decoded source chunk.
    ///
    /// Returns the original input when it requires splitting into prepared
    /// quanta. The caller retains the unconsumed suffix between operations.
    pub fn render(&mut self, mut chunk: AudioChunk) -> ControlFlow<AudioChunk, Option<AudioChunk>> {
        if self.transition_pending() {
            return ControlFlow::Break(chunk);
        }
        if self.projection.active.is_some() || self.projection.selected.is_some() {
            let frames = self.prepare_quantum(chunk.meta, chunk.frames());
            if !frames.is_ok_and(|frames| frames.get() == chunk.frames()) {
                return ControlFlow::Break(chunk);
            }
            return self.render_quantum(chunk);
        }
        let snapshot = self.context.load();
        self.prepared_quantum = None;
        let rate = self.controls.rate_target();
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

        let output = if let Some(prepared) = prepared.filter(|quantum| quantum.projection.is_some())
        {
            match self.render_resident_projection(chunk, prepared) {
                Ok(output) => output,
                Err(error) => {
                    warn!(%error, "resident projection rendering failed");
                    return None;
                }
            }
        } else {
            self.render_manual(chunk, speed, prepared)
        };
        if let Some(output) = output.as_ref() {
            self.commit_rate_render(
                snapshot,
                output,
                speed,
                target_speed,
                prepared.and_then(|quantum| quantum.projection),
            );
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
        let mut chunk = chunk;
        if let Some(prepared) = prepared
            && let Err(error) = self.activate_prepared_quantum(&mut chunk, prepared)
        {
            warn!(%error, "time-stretch activation failed; dropping chunk");
            self.retire_engine();
            self.clear_render_state();
            self.defer_scratch(Some(chunk.samples));
            return None;
        }
        self.process_active(chunk, speed)
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
        chunk.meta.mapping_revision = prepared
            .projection
            .and_then(|projection| std::num::NonZeroU64::new(u64::from(projection.end.revision())));
        let output = self.render_at(
            chunk,
            prepared.speed,
            snapshot,
            Some(prepared),
            prepared.rate.speed(),
        );
        if output.is_some()
            && let Some(projection) = prepared.projection
        {
            self.accept_projected_output(projection);
        }
        ControlFlow::Continue(output)
    }

    pub(super) fn accept_projected_output(
        &mut self,
        projection: super::renderer_projection::ProjectedQuantum,
    ) {
        if let Some(plan) = self.projection.prepared.take() {
            let same = self
                .projection
                .active
                .as_ref()
                .is_some_and(|active| kithara_platform::sync::Arc::ptr_eq(active, &plan));
            if !same {
                debug_assert!(self.projection.retired.is_none());
                self.projection.retired = self.projection.active.replace(plan);
            }
        }
        self.projection.cursor = Some(projection.end);
        self.projection.output_frames = projection
            .output_offset
            .saturating_add(projection.output_frames);
    }

    /// Discard renderer state after a source discontinuity.
    pub fn reset(&mut self) {
        self.projection.cursor = None;
        self.projection.output_frames = 0;
        self.reset_pending = true;
        self.clear_render_state();
        self.committed = None;
        self.snap_speed();
    }
}

impl<S: HasPool<f32>> WarpRenderer<S> {
    fn retain_projected_replacement(&mut self) -> Result<(), ElasticError> {
        let channels = usize::from(self.spec.channels.max(1));
        let capacity = self
            .residency
            .as_ref()
            .ok_or(ElasticError::PoolCapacity)?
            .replacement
            .capacity()
            / channels;
        let quantum = self
            .engine
            .as_ref()
            .ok_or(ElasticError::EnginePreparation(
                "projected engine is unavailable",
            ))?
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
            let output = self.scratch.as_ref().ok_or(ElasticError::PoolCapacity)?;
            if resident.replacement.len() + output.len() > resident.replacement.capacity() {
                return Err(ElasticError::PoolCapacity);
            }
            resident
                .replacement
                .try_extend_from_slice(output)
                .map_err(|_| ElasticError::PoolCapacity)?;
            self.scratch
                .as_mut()
                .ok_or(ElasticError::PoolCapacity)?
                .clear();
            if complete {
                resident.primed = false;
                self.backend_transition_pending = false;
                self.active = false;
                self.applied_pitch = f64::NAN;
                self.reset_pending = false;
                return Ok(());
            }
        }
        Err(ElasticError::EnginePreparation(
            "projected backend drain exceeds its capability bound",
        ))
    }
}

impl<S: HasPool<f32>> WarpRenderer<S> {
    fn flush_resident_request(
        &mut self,
        snapshot: Option<crate::RenderSnapshot>,
    ) -> Option<AudioChunk> {
        let request = self.residency.as_ref()?.prepared?;
        let channels = usize::from(self.spec.channels.max(1));
        let result = (|| {
            let resident = self.residency.as_mut().ok_or(ElasticError::PoolCapacity)?;
            resident.pad_to(request.source_end, channels)?;
            // EOF silence supplies DSP lookahead, never new recording geometry.
            let meta = self.last_input_meta.ok_or(ElasticError::EmptySource)?;
            self.process_resident_projection(meta, channels)
        })();
        match result {
            Ok(Some(mut output)) => {
                let projection = match self.trim_projected_eof(&mut output, request.projection) {
                    Ok(projection) => projection,
                    Err(error) => {
                        warn!(%error, "projected EOF identity is uncovered");
                        self.defer_scratch(Some(output.samples));
                        return None;
                    }
                };
                self.commit_rate_render(
                    snapshot,
                    &output,
                    1.0,
                    request.rate.speed(),
                    Some(projection),
                );
                self.accept_projected_output(projection);
                Some(output)
            }
            Ok(None) => None,
            Err(error) => {
                warn!(%error, "projected EOF completion failed");
                None
            }
        }
    }

    fn trim_projected_eof(
        &mut self,
        output: &mut AudioChunk,
        mut projection: super::renderer_projection::ProjectedQuantum,
    ) -> Result<super::renderer_projection::ProjectedQuantum, ElasticError> {
        let plan = self
            .projection
            .prepared
            .as_ref()
            .or(self.projection.active.as_ref())
            .ok_or(ElasticError::EnginePreparation("projected EOF has no plan"))?;
        let end = self
            .residency
            .as_ref()
            .and_then(|resident| resident.end)
            .ok_or(ElasticError::EmptySource)?;
        let source = end
            .to_f64()
            .and_then(|end| crate::AssetFrame::new(end).ok())
            .ok_or(ElasticError::SampleCountOverflow)?;
        let crate::BeatGridQuery::Resolved(end_output) = plan.map().output_at(source) else {
            return Err(ElasticError::EnginePreparation(
                "projected source EOF is uncovered",
            ));
        };
        let axis = plan.output_axis().ok_or(ElasticError::EnginePreparation(
            "projected EOF has no session axis",
        ))?;
        let total = self.projected_output_offset(plan, end_output)?;
        let frames = output
            .frames()
            .min(total.saturating_sub(projection.output_offset));
        let total_frames = projection
            .output_offset
            .checked_add(frames)
            .ok_or(ElasticError::SampleCountOverflow)?;
        let output_offset = (total_frames
            .to_f64()
            .ok_or(ElasticError::SampleCountOverflow)?
            * f64::from(axis.sample_rate().get())
            / f64::from(self.spec.sample_rate.get()))
        .round()
        .to_i64()
        .ok_or(ElasticError::SampleCountOverflow)?;
        let output_end = crate::SessionFrame::new(
            i64::from(plan.activation().output())
                .checked_add(output_offset)
                .ok_or(ElasticError::SampleCountOverflow)?,
        );
        let source = Self::projected_endpoint(plan, output_end, end)?;
        projection.output_frames = frames;
        projection.end = plan.map().reanchor(source, output_end);
        output
            .samples
            .truncate(frames * usize::from(self.spec.channels.max(1)));
        output.meta.frames =
            u32::try_from(frames).map_err(|_| ElasticError::SampleCountOverflow)?;
        output.meta.frame_offset = Self::projected_endpoint(plan, projection.output_start, end)?;
        output.meta.timestamp = self
            .spec
            .duration_for(output.meta.frame_offset)
            .map_err(|_| ElasticError::SampleCountOverflow)?;
        output.meta.end_timestamp = self
            .spec
            .duration_for(source)
            .map_err(|_| ElasticError::SampleCountOverflow)?;
        output.meta.mapping_revision =
            std::num::NonZeroU64::new(u64::from(projection.end.revision()));
        self.rendered_source_end = Some((source, self.spec.sample_rate));
        Ok(projection)
    }
}
