use std::ops::ControlFlow;

use kithara_audio::{
    AudioSource, Fetch, SourceDiscontinuity, SourceEnd, TrackFailureKind, TrackStep, WaitingReason,
};
use kithara_bufpool::{HasPool, PoolRegion, SampleBuffer};
use kithara_command::Inbox;
use kithara_effects::{
    AudioEffect, EffectDrain, EffectDrainStep, apply_effects, held_source_frames, reset_effects,
};
use kithara_platform::sync::Arc;
use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec, FrameCount};
use kithara_stream::SeekObserve;
use kithara_warp::WarpRenderError;

use crate::{LaneProtocol, lane::Lane};

#[derive(Clone, Copy)]
pub(super) enum DrainState {
    Open,
    LiveWarp(u64),
    Warp(u64),
    Effects(u64),
    Exhausted(u64),
}

impl DrainState {
    const fn epoch(self) -> Option<u64> {
        match self {
            Self::Open => None,
            Self::LiveWarp(epoch)
            | Self::Warp(epoch)
            | Self::Effects(epoch)
            | Self::Exhausted(epoch) => Some(epoch),
        }
    }
}

pub(super) struct PendingInput {
    pub(super) chunk: AudioChunk,
    pub(super) epoch: u64,
    pub(super) consumed_frames: usize,
}

/// The sole producer-side Warp/effect stage before the play output ring.
pub struct WarpSource<T, S> {
    seek: Arc<dyn SeekObserve>,
    spec: AudioSpec,
    pub(super) drain_state: DrainState,
    drain: EffectDrain,
    discontinuity: Option<SourceDiscontinuity>,
    lane: Lane,
    pub(super) pending_input: Option<PendingInput>,
    pub(super) prepared_frames: Option<usize>,
    pub(super) render_input: Option<SampleBuffer>,
    reset_epoch: Option<u64>,
    pub(super) retired_input: Option<AudioChunk>,
    staged_epoch: Option<u64>,
    staged_meta: Option<AudioChunkInfo>,
    pools: PoolRegion<S>,
    pub(super) source: T,
    pub(super) effects: Vec<Box<dyn AudioEffect>>,
    pub(super) warp: kithara_warp::WarpRenderer<S>,
    pub(super) quantum_failed: bool,
}

impl<T, S> WarpSource<T, S>
where
    T: AudioSource<Chunk = AudioChunk>,
    S: HasPool<f32>,
{
    /// Builds the stage over a decoded source, its Warp renderer, and the
    /// effect chain with the drain that flushes it; `inbox` brings the lane
    /// commands it executes at frames of its output.
    pub fn new(
        source: T,
        warp: kithara_warp::WarpRenderer<S>,
        effects: Vec<Box<dyn AudioEffect>>,
        drain: EffectDrain,
        spec: AudioSpec,
        pools: PoolRegion<S>,
        inbox: Inbox<LaneProtocol>,
    ) -> Self {
        let discontinuity = source.discontinuity();
        let seek = source.seek_observe();
        Self {
            source,
            warp,
            effects,
            drain,
            seek,
            discontinuity,
            spec,
            pools,
            lane: Lane::new(inbox),
            drain_state: DrainState::Open,
            reset_epoch: None,
            pending_input: None,
            staged_meta: None,
            staged_epoch: None,
            prepared_frames: None,
            render_input: None,
            retired_input: None,
            quantum_failed: false,
        }
    }

    fn clear_staging(&mut self) {
        if let Some(input) = self.render_input.as_mut() {
            input.clear();
        }
        self.staged_meta = None;
        self.staged_epoch = None;
        self.prepared_frames = None;
    }

    fn discard_staged_input(&mut self) {
        self.retire_pending_input();
        self.clear_staging();
        self.quantum_failed = false;
    }

    pub(super) fn prepare_staging(&mut self) {
        if self.quantum_failed {
            return;
        }
        if self.prepared_frames.is_some() {
            return;
        }
        let Some((meta, remaining)) = self.pending_input.as_ref().and_then(|pending| {
            let remaining = pending
                .chunk
                .frames()
                .checked_sub(pending.consumed_frames)?;
            Some((
                Self::span_meta(pending.chunk.meta, pending.consumed_frames, remaining)?,
                remaining,
            ))
        }) else {
            return;
        };
        let frames = match self.prepare_quantum(meta, remaining) {
            Ok(frames) => frames,
            Err(WarpRenderError::PendingActivation) => return,
            Err(WarpRenderError::Preroll { frames }) => {
                if self.admit_preroll(frames.get().min(remaining)) {
                    self.prepare_staging();
                }
                return;
            }
            Err(WarpRenderError::NeedsService) => {
                if self.warp.transition_pending() {
                    self.drain_state = DrainState::LiveWarp(self.source.decode_epoch());
                }
                return;
            }
            Err(_) => {
                self.quantum_failed = true;
                return;
            }
        };
        let frames = frames.get();
        let Some(required) = frames.checked_mul(usize::from(self.spec.channels.max(1))) else {
            self.quantum_failed = true;
            return;
        };

        let mut input = self
            .render_input
            .take()
            .unwrap_or_else(|| self.pools.get::<f32>());
        if input.ensure_len(required).is_err() {
            self.render_input = Some(input);
            self.quantum_failed = true;
            return;
        }
        input.clear();
        self.render_input = Some(input);
        if frames == 0 {
            self.staged_meta = Self::span_meta(meta, 0, 0);
            self.staged_epoch = self.pending_input.as_ref().map(|pending| pending.epoch);
        }
        self.prepared_frames = Some(frames);
    }

    /// Hands the next `frames` of pending input to an entered renderer as
    /// history before its activation. Returns whether input remains to stage.
    fn admit_preroll(&mut self, frames: usize) -> bool {
        let channels = usize::from(self.spec.channels.max(1));
        let Some(pending) = self.pending_input.as_mut() else {
            return false;
        };
        let start = pending.consumed_frames.saturating_mul(channels);
        let end = start.saturating_add(frames.saturating_mul(channels));
        let span = Self::span_meta(pending.chunk.meta, pending.consumed_frames, frames);
        let admitted = match (span, pending.chunk.samples.get(start..end)) {
            (Some(span), Some(samples)) => self.warp.admit_preroll(span, samples).is_ok(),
            _ => false,
        };
        if !admitted {
            self.quantum_failed = true;
            return false;
        }
        pending.consumed_frames = pending.consumed_frames.saturating_add(frames);
        if pending.consumed_frames < pending.chunk.frames() {
            return true;
        }
        if let Some(pending) = self.pending_input.take() {
            self.source.retire_chunk(pending.chunk);
        }
        false
    }

    fn retire_pending_input(&mut self) {
        let Some(pending) = self.pending_input.take() else {
            return;
        };
        debug_assert!(self.retired_input.is_none());
        self.retired_input = Some(pending.chunk);
    }

    /// Where a lane entering its plan must start decoding: the renderer's
    /// entry source as a position in the source's own timeline.
    pub fn entry_position(&self) -> Option<kithara_platform::time::Duration> {
        let entry = self.warp.entry_source()?;
        self.spec.duration_for(entry).ok()
    }

    fn span_meta(original: AudioChunkInfo, offset: usize, frames: usize) -> Option<AudioChunkInfo> {
        let offset = u64::try_from(offset).ok()?;
        let frames = u32::try_from(frames).ok()?;
        let mut meta = original;
        meta.frame_offset = original.frame_offset.checked_add(offset)?;
        meta.timestamp = original
            .timestamp
            .checked_add(original.spec.duration_for(offset).ok()?)?;
        meta.frames = frames;
        meta.end_timestamp = meta
            .timestamp
            .checked_add(original.spec.duration_for(u64::from(frames)).ok()?)?;
        meta.source_byte_offset = None;
        meta.source_bytes = 0;
        Some(meta)
    }

    pub(super) fn stage_pending(&mut self) -> bool {
        let channels = usize::from(self.spec.channels.max(1));
        let Some(capacity) = self
            .prepared_frames
            .and_then(|frames| frames.checked_mul(channels))
        else {
            return false;
        };
        let staged = self.render_input.as_ref().map_or(0, |input| input.len());
        let staged_frames = staged / channels;
        let Some(pending) = self.pending_input.as_mut() else {
            return false;
        };
        if pending.chunk.spec() != self.spec || pending.epoch != self.seek.epoch() {
            self.quantum_failed = true;
            return false;
        }

        let pending_frame = u64::try_from(pending.consumed_frames)
            .ok()
            .and_then(|consumed| pending.chunk.meta.frame_offset.checked_add(consumed));
        let expected_frame = self.staged_meta.and_then(|meta| {
            meta.frame_offset
                .checked_add(u64::try_from(staged_frames).ok()?)
        });
        if staged > 0
            && (self.staged_epoch != Some(pending.epoch) || expected_frame != pending_frame)
        {
            self.quantum_failed = true;
            return false;
        }

        let remaining_frames = pending
            .chunk
            .frames()
            .saturating_sub(pending.consumed_frames);
        let free_frames = capacity.saturating_sub(staged) / channels;
        let frames = remaining_frames.min(free_frames);
        let source_start = pending.consumed_frames.saturating_mul(channels);
        let samples = frames.saturating_mul(channels);
        let source_end = source_start.saturating_add(samples);
        let Some(source) = pending.chunk.samples.get(source_start..source_end) else {
            self.quantum_failed = true;
            return false;
        };
        if staged == 0 {
            self.staged_meta = Self::span_meta(pending.chunk.meta, pending.consumed_frames, 0);
            self.staged_epoch = Some(pending.epoch);
        }
        let Some(input) = self.render_input.as_mut() else {
            self.quantum_failed = true;
            return false;
        };
        let end = staged.saturating_add(samples);
        if end > input.capacity() || input.try_extend_from_slice(source).is_err() {
            self.quantum_failed = true;
            return false;
        }
        pending.consumed_frames = pending.consumed_frames.saturating_add(frames);
        let consumed = pending.consumed_frames == pending.chunk.frames();
        if consumed {
            self.retire_pending_input();
        }
        consumed
    }

    fn staged_frames(&self) -> usize {
        let channels = usize::from(self.spec.channels.max(1));
        self.render_input.as_ref().map_or(0, |input| input.len()) / channels
    }
}

impl<T, S> WarpSource<T, S>
where
    T: AudioSource<Chunk = AudioChunk>,
    S: HasPool<f32>,
{
    fn begin_drain(&mut self, epoch: u64) {
        self.drain_state = DrainState::Warp(epoch);
    }

    fn cancel_stale_drain(&mut self) -> bool {
        let stale = self
            .drain_state
            .epoch()
            .is_some_and(|epoch| self.seek.epoch() != epoch || self.source.decode_epoch() != epoch);
        if !stale {
            return false;
        }
        let epoch = self.seek.epoch();
        self.reset_renderers();
        self.drain.reset();
        self.drain_state = DrainState::Open;
        self.reset_epoch = Some(epoch);
        true
    }

    fn cancel_stale_input(&mut self) -> bool {
        let stale = self.pending_input.as_ref().is_some_and(|pending| {
            self.seek.epoch() != pending.epoch || self.source.decode_epoch() != pending.epoch
        }) || self
            .staged_epoch
            .is_some_and(|epoch| self.seek.epoch() != epoch || self.source.decode_epoch() != epoch);
        if !stale {
            return false;
        }
        let epoch = self.seek.epoch();
        self.discard_staged_input();
        self.reset_renderers();
        self.drain.reset();
        self.drain_state = DrainState::Open;
        self.reset_epoch = Some(epoch);
        true
    }

    fn drain_step(&mut self) -> Option<TrackStep<AudioChunk>> {
        if let DrainState::LiveWarp(epoch) = self.drain_state {
            let chunk = self.warp.flush();
            if !self.warp.transition_pending() {
                self.drain_state = DrainState::Open;
            }
            return Some(
                chunk
                    .and_then(|chunk| apply_effects(&mut self.effects, chunk))
                    .map_or(TrackStep::StateChanged, |output| {
                        TrackStep::Produced(self.fetch(output, epoch))
                    }),
            );
        }

        if let DrainState::Warp(epoch) = self.drain_state {
            if let Some(chunk) = self.warp.flush() {
                return Some(
                    apply_effects(&mut self.effects, chunk)
                        .map_or(TrackStep::StateChanged, |output| {
                            TrackStep::Produced(self.fetch(output, epoch))
                        }),
                );
            }
            self.drain_state = DrainState::Effects(epoch);
        }

        let DrainState::Effects(epoch) = self.drain_state else {
            return None;
        };
        Some(match self.drain.step(&mut self.effects) {
            EffectDrainStep::Produced(chunk) => TrackStep::Produced(self.fetch(chunk, epoch)),
            EffectDrainStep::Progress => TrackStep::StateChanged,
            EffectDrainStep::Exhausted => {
                self.drain_state = DrainState::Exhausted(epoch);
                TrackStep::Eof
            }
        })
    }

    fn fetch(&mut self, data: AudioChunk, epoch: u64) -> Fetch<AudioChunk> {
        self.lane.advance(data.frames());
        let source_end = self.warp.rendered_source_end().map(|(frame, sample_rate)| {
            SourceEnd::new(
                frame.saturating_sub(held_source_frames(&self.effects)),
                sample_rate,
            )
        });
        match source_end {
            Some(source_end) => Fetch::rendered(data, epoch, source_end),
            None => Fetch::data(data, epoch),
        }
    }

    /// Executes the lane batches due at its cursor, then prepares the quantum
    /// that starts there, ending it at the next batch's frame.
    fn prepare_quantum(
        &mut self,
        meta: AudioChunkInfo,
        remaining: usize,
    ) -> Result<FrameCount, WarpRenderError> {
        self.lane.execute_due(&mut self.warp);
        self.warp
            .prepare_quantum(meta, remaining, self.lane.output_limit())
    }

    fn prepare_renderers(&mut self, spec: AudioSpec) {
        self.spec = spec;
        self.warp.prepare(spec);
        if self.warp.transition_pending() && matches!(self.drain_state, DrainState::Open) {
            self.drain_state = DrainState::LiveWarp(self.source.decode_epoch());
        }
        if !self.warp.transition_pending() {
            self.prepare_staging();
        }
        for effect in &mut self.effects {
            effect.service_deferred(spec);
        }
    }

    fn render_full_quantum(&mut self) -> Option<TrackStep<AudioChunk>> {
        let frames = self.prepared_frames?;
        (self.staged_frames() == frames).then(|| self.render_staged(frames))
    }

    fn render_quantum(
        &mut self,
        chunk: AudioChunk,
        epoch: u64,
    ) -> ControlFlow<AudioChunk, Option<Fetch<AudioChunk>>> {
        let output = self.warp.render_quantum(chunk)?;
        if self.warp.transition_pending() {
            self.drain_state = DrainState::LiveWarp(epoch);
        }
        let output = output.and_then(|chunk| apply_effects(&mut self.effects, chunk));
        ControlFlow::Continue(output.map(|output| self.fetch(output, epoch)))
    }

    fn render_source_quantum(
        &mut self,
        chunk: AudioChunk,
        epoch: u64,
    ) -> Option<Fetch<AudioChunk>> {
        match self.render_quantum(chunk, epoch) {
            ControlFlow::Continue(output) => output,
            ControlFlow::Break(input) => {
                debug_assert!(self.retired_input.is_none());
                self.retired_input = Some(input);
                self.quantum_failed = true;
                None
            }
        }
    }

    pub(super) fn render_staged(&mut self, frames: usize) -> TrackStep<AudioChunk> {
        if self.quantum_failed {
            return TrackStep::Failed(TrackFailureKind::Render);
        }
        let channels = usize::from(self.spec.channels.max(1));
        let Some(samples) = frames.checked_mul(channels) else {
            self.quantum_failed = true;
            return TrackStep::Failed(TrackFailureKind::Render);
        };
        let Some(meta) = self
            .staged_meta
            .and_then(|meta| Self::span_meta(meta, 0, frames))
        else {
            self.quantum_failed = true;
            return TrackStep::Failed(TrackFailureKind::Render);
        };
        let Some(epoch) = self.staged_epoch else {
            self.quantum_failed = true;
            return TrackStep::Failed(TrackFailureKind::Render);
        };
        let Some(input) = self.render_input.take() else {
            return TrackStep::StateChanged;
        };
        if input.len() != samples {
            self.render_input = Some(input);
            self.quantum_failed = true;
            return TrackStep::Failed(TrackFailureKind::Render);
        }
        self.staged_meta = None;
        self.staged_epoch = None;
        self.prepared_frames = None;
        match self.render_quantum(AudioChunk::new(meta, input), epoch) {
            ControlFlow::Continue(output) => {
                output.map_or(TrackStep::StateChanged, TrackStep::Produced)
            }
            ControlFlow::Break(input) => {
                self.render_input = Some(input.samples);
                self.quantum_failed = true;
                TrackStep::Failed(TrackFailureKind::Render)
            }
        }
    }

    fn render_whole_pending(&mut self) -> Option<TrackStep<AudioChunk>> {
        let prepared = self.prepared_frames?;
        let pending = self.pending_input.as_ref()?;
        if pending.consumed_frames != 0 || pending.chunk.frames() != prepared {
            return None;
        }
        let pending = self.pending_input.take()?;
        self.prepared_frames = None;
        Some(
            self.render_source_quantum(pending.chunk, pending.epoch)
                .map_or(TrackStep::StateChanged, TrackStep::Produced),
        )
    }

    fn reset_renderers(&mut self) {
        self.warp.reset();
        reset_effects(&mut self.effects);
    }

    fn sync_discontinuity(&mut self) {
        let next = self.source.discontinuity();
        let revision_changed = next.as_ref().map(SourceDiscontinuity::revision)
            != self
                .discontinuity
                .as_ref()
                .map(SourceDiscontinuity::revision);
        if let Some(discontinuity) = next.as_ref() {
            self.spec = *discontinuity.spec();
        }
        self.discontinuity = next;
        let already_reset = self.reset_epoch == Some(self.source.decode_epoch());
        if !revision_changed {
            return;
        }
        self.discard_staged_input();
        if already_reset {
            self.reset_epoch = None;
        } else {
            self.reset_renderers();
        }
        self.drain.reset();
        self.drain_state = DrainState::Open;
    }
}

impl<T, S> AudioSource for WarpSource<T, S>
where
    T: AudioSource<Chunk = AudioChunk>,
    S: HasPool<f32> + Send + Sync + 'static,
{
    type Chunk = AudioChunk;

    fn discontinuity(&self) -> Option<SourceDiscontinuity> {
        self.discontinuity
    }

    fn prepare_deferred(&mut self) -> Option<AudioSpec> {
        if let Some(chunk) = self.retired_input.take() {
            self.source.retire_chunk(chunk);
        }
        let spec = self.source.prepare_deferred();
        self.sync_discontinuity();
        self.prepare_renderers(spec.unwrap_or(self.spec));
        spec
    }

    fn seek_observe(&self) -> Arc<dyn SeekObserve> {
        Arc::clone(&self.seek)
    }

    fn step_track(&mut self) -> TrackStep<AudioChunk> {
        self.sync_discontinuity();
        if self.cancel_stale_input() {
            return TrackStep::StateChanged;
        }
        if self.cancel_stale_drain() {
            return TrackStep::StateChanged;
        }
        if self.quantum_failed {
            return TrackStep::Failed(TrackFailureKind::Render);
        }

        if matches!(self.drain_state, DrainState::Exhausted(_)) {
            return TrackStep::Eof;
        }
        if let Some(step) = self.drain_step() {
            return step;
        }
        if let Some(step) = self.render_whole_pending() {
            return step;
        }
        if let Some(step) = self.render_full_quantum() {
            return step;
        }
        if self.pending_input.is_some() {
            if self.prepared_frames.is_none() {
                return TrackStep::Blocked(WaitingReason::Waiting);
            }
            self.stage_pending();
            return self
                .render_full_quantum()
                .unwrap_or(TrackStep::StateChanged);
        }
        if !self.warp.accepts_input() {
            return TrackStep::Failed(TrackFailureKind::Render);
        }

        match self.source.step_track() {
            TrackStep::Produced(Fetch::Data { data, epoch, .. }) => {
                if data.spec() == self.spec
                    && self.prepared_frames.is_none()
                    && self
                        .prepare_quantum(data.meta, data.frames())
                        .is_ok_and(|frames| frames.get() == data.frames())
                {
                    return self
                        .render_source_quantum(data, epoch)
                        .map_or(TrackStep::StateChanged, TrackStep::Produced);
                }
                self.pending_input = Some(PendingInput {
                    epoch,
                    chunk: data,
                    consumed_frames: 0,
                });
                TrackStep::StateChanged
            }
            TrackStep::Produced(fetch) => TrackStep::Produced(fetch),
            TrackStep::Eof => {
                self.begin_drain(self.source.decode_epoch());
                let frames = self.staged_frames();
                if frames == 0 {
                    TrackStep::StateChanged
                } else {
                    let Some(meta) = self.staged_meta else {
                        self.quantum_failed = true;
                        return TrackStep::Failed(TrackFailureKind::Render);
                    };
                    let Some(frames) = self.warp.prepare_terminal_quantum(meta, frames) else {
                        self.quantum_failed = true;
                        return TrackStep::Failed(TrackFailureKind::Render);
                    };
                    self.prepared_frames = Some(frames.get());
                    self.render_staged(frames.get())
                }
            }
            TrackStep::StateChanged => {
                self.sync_discontinuity();
                TrackStep::StateChanged
            }
            TrackStep::Blocked(reason) => TrackStep::Blocked(reason),
            TrackStep::Failed(failure) => TrackStep::Failed(failure),
        }
    }

    delegate::delegate! {
        to self.source {
            fn decode_epoch(&self) -> u64;
            fn commit_source_end(&mut self, source_end: SourceEnd, epoch: u64);
            fn retire_chunk(&self, chunk: AudioChunk);
            fn finish_deferred(&mut self);
            fn warm_up(&mut self);
        }
    }
}
