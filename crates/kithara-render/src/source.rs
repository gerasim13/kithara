use std::{
    num::NonZeroU32,
    ops::ControlFlow,
    task::{Context, Poll},
};

use kithara_audio::{
    AudioReadError, AudioSource, Fetch, SeekOutcome, SourceDiscontinuity, SourceEnd,
    TrackFailureKind, TrackStep, WaitingReason,
};
use kithara_bufpool::{HasPool, PoolRegion, SampleBuffer};
use kithara_effects::{AudioEffect, EffectDrain, EffectDrainStep, apply_effects, reset_effects};
use kithara_platform::time::Duration;
use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec, FrameCount};
use kithara_warp::WarpRenderError;

use crate::{
    LaneFrame, LaneSetup,
    lane::{Lane, LaneChange},
};

#[derive(Clone, Copy)]
enum DrainState {
    Open,
    LiveWarp,
    Warp,
    Effects,
    Exhausted,
}

struct PendingInput {
    chunk: AudioChunk,
    consumed_frames: usize,
}

/// The sole producer-side Warp/effect stage before the play output ring.
pub struct WarpSource<T, S> {
    spec: AudioSpec,
    drain_state: DrainState,
    drain: EffectDrain,
    discontinuity: Option<SourceDiscontinuity>,
    lane: Lane,
    pending_input: Option<PendingInput>,
    prepared_frames: Option<usize>,
    render_input: Option<SampleBuffer>,
    retired_input: Option<AudioChunk>,
    staged_meta: Option<AudioChunkInfo>,
    pools: PoolRegion<S>,
    source: T,
    effects: Vec<Box<dyn AudioEffect>>,
    warp: kithara_warp::WarpRenderer<S>,
    quantum_failed: bool,
    terminal_failure: Option<TrackFailureKind>,
}

impl<T, S> WarpSource<T, S>
where
    T: AudioSource<Chunk = AudioChunk>,
    S: HasPool<f32>,
{
    /// Builds the stage over a decoded source, its Warp renderer, and the
    /// effect chain with the drain that flushes it; `lane` brings the lane
    /// commands it executes at frames of its output.
    pub fn new(
        source: T,
        warp: kithara_warp::WarpRenderer<S>,
        effects: Vec<Box<dyn AudioEffect>>,
        drain: EffectDrain,
        spec: AudioSpec,
        pools: PoolRegion<S>,
        lane: LaneSetup,
    ) -> Self {
        let discontinuity = source.discontinuity();
        Self {
            source,
            warp,
            effects,
            drain,
            discontinuity,
            spec,
            pools,
            lane: Lane::new(lane.inbox, lane.preload_chunks, lane.declick),
            drain_state: DrainState::Open,
            pending_input: None,
            staged_meta: None,
            prepared_frames: None,
            render_input: None,
            retired_input: None,
            quantum_failed: false,
            terminal_failure: None,
        }
    }

    delegate::delegate! {
        to self.lane {
            /// Current segment-relative output frame.
            #[must_use]
            pub const fn cursor(&self) -> LaneFrame;
            /// Exact source position represented by the lane output cursor, when established.
            #[must_use]
            pub const fn position(&self) -> Option<Duration>;
            /// Records successful admission of the current segment's chunk.
            pub fn admitted(&mut self);
            pub(crate) fn upstream_parked(&mut self);
            pub(crate) fn finish_preload(&mut self);
            pub(crate) fn finish_segment(&mut self);
            /// Whether admission has made the current segment ready for playback.
            #[must_use]
            pub fn is_preloaded(&self) -> bool;
            /// Registers the owning task's wake for lane command arrivals.
            pub fn poll_commands(&mut self, context: &mut Context<'_>) -> Poll<()>;
        }
    }

    /// Exact output latency of the prepared engine.
    #[must_use]
    pub fn engine_latency(&self) -> FrameCount {
        self.warp.engine_latency()
    }

    /// Output frames of this lane's Jump ramp at its current sample rate.
    #[must_use]
    pub fn declick_frames(&self) -> FrameCount {
        self.lane.declick_frames(self.spec.sample_rate)
    }

    /// Executes commands at the output cursor before producing more samples.
    /// Returns whether a command reset the decoded source.
    ///
    /// # Errors
    /// Returns the source's seek classification or a render failure.
    pub fn service_commands(&mut self) -> Result<bool, TrackFailureKind> {
        let changed = self
            .lane
            .execute_due(&mut self.source, &mut self.warp, self.spec)?;
        if changed == LaneChange::Source {
            self.discard_staged_input();
            reset_effects(&mut self.effects);
            self.drain.reset();
            self.drain_state = DrainState::Open;
            self.discontinuity = self.source.discontinuity();
            self.spec = self.discontinuity.map_or(self.spec, |stamp| *stamp.spec());
        } else if changed == LaneChange::Controls {
            self.prepared_frames = None;
        }
        self.prepare_renderers(self.spec);
        Ok(changed == LaneChange::Source)
    }

    fn fail(&mut self, failure: TrackFailureKind) -> TrackStep<AudioChunk> {
        TrackStep::Failed(*self.terminal_failure.get_or_insert(failure))
    }

    fn clear_staging(&mut self) {
        if let Some(input) = self.render_input.as_mut() {
            input.clear();
        }
        self.staged_meta = None;
        self.prepared_frames = None;
    }

    fn discard_staged_input(&mut self) {
        self.retire_pending_input();
        self.clear_staging();
        self.quantum_failed = false;
    }

    fn prepare_staging(&mut self) {
        if self.quantum_failed || self.lane.output_limit() == 0 {
            return;
        }
        let pending_span = self.pending_input.as_ref().and_then(|pending| {
            let remaining = pending
                .chunk
                .frames()
                .checked_sub(pending.consumed_frames)?;
            Some((
                Self::span_meta(pending.chunk.meta, pending.consumed_frames, remaining)?,
                remaining,
            ))
        });
        let staged = self.staged_frames();
        let span = self.staged_meta.map_or(pending_span, |meta| {
            Some((
                meta,
                staged.saturating_add(pending_span.map_or(0, |(_, remaining)| remaining)),
            ))
        });
        let Some((meta, remaining)) = span else {
            return;
        };
        let frames = match self.prepare_quantum(meta, remaining) {
            Ok(frames) => frames,
            Err(WarpRenderError::NeedsService) => {
                if self.warp.transition_pending() {
                    self.drain_state = DrainState::LiveWarp;
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
        let staged_samples = input.len();
        if input.ensure_len(required.max(staged_samples)).is_err() {
            self.render_input = Some(input);
            self.quantum_failed = true;
            return;
        }
        input.truncate(staged_samples);
        self.render_input = Some(input);
        if frames == 0 {
            self.staged_meta = Self::span_meta(meta, 0, 0);
        }
        self.prepared_frames = Some(frames);
    }

    fn retire_pending_input(&mut self) {
        let Some(pending) = self.pending_input.take() else {
            return;
        };
        if let Some(chunk) = self.retired_input.take() {
            self.source.retire_chunk(chunk);
        }
        self.retired_input = Some(pending.chunk);
    }

    fn span_meta(original: AudioChunkInfo, offset: usize, frames: usize) -> Option<AudioChunkInfo> {
        let offset = u64::try_from(offset).ok()?;
        let frames = u32::try_from(frames).ok()?;
        let mut meta = original;
        if let Some(span) = original.source_span {
            let end = offset.checked_add(u64::from(frames))?;
            let span = span.for_output_range(offset..end)?;
            meta.frame_offset = span.start();
            meta.timestamp = span.position_at(0)?;
            meta.end_timestamp = span.position_at(span.output_frames())?;
            meta.frames = frames;
            meta.source_span = Some(span);
            meta.source_byte_offset = None;
            meta.source_bytes = 0;
            meta.end_of_track = original.end_of_track && end == u64::from(original.frames);
            return Some(meta);
        }
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
        meta.end_of_track = original.end_of_track
            && offset.checked_add(u64::from(frames))? == u64::from(original.frames);
        Some(meta)
    }

    fn stage_pending(&mut self) -> bool {
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
        if pending.chunk.spec() != self.spec {
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
        if staged > 0 && expected_frame != pending_frame {
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
            self.staged_meta = Self::span_meta(pending.chunk.meta, pending.consumed_frames, frames);
        } else if let Some(meta) = self.staged_meta.as_mut() {
            let Some(next) = Self::span_meta(pending.chunk.meta, pending.consumed_frames, frames)
            else {
                self.quantum_failed = true;
                return false;
            };
            if let Some(span) = meta.source_span {
                let Some(span) = next.source_span.and_then(|next| span.followed_by(next)) else {
                    self.quantum_failed = true;
                    return false;
                };
                meta.source_span = Some(span);
            }
            meta.end_timestamp = next.end_timestamp;
            let Ok(total) = u32::try_from(staged_frames.saturating_add(frames)) else {
                self.quantum_failed = true;
                return false;
            };
            meta.frames = total;
            meta.end_of_track = next.end_of_track;
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
    fn begin_drain(&mut self) {
        self.drain_state = DrainState::Warp;
    }

    fn drain_step(&mut self) -> Option<TrackStep<AudioChunk>> {
        if let DrainState::LiveWarp = self.drain_state {
            let Ok(chunk) = self.warp.drain(self.lane.output_limit()) else {
                self.quantum_failed = true;
                return Some(TrackStep::StateChanged);
            };
            if !self.warp.transition_pending() {
                self.drain_state = DrainState::Open;
            }
            return Some(
                chunk
                    .and_then(|chunk| apply_effects(&mut self.effects, chunk))
                    .and_then(|output| self.fetch(output))
                    .map_or(TrackStep::StateChanged, TrackStep::Produced),
            );
        }

        if let DrainState::Warp = self.drain_state {
            let Ok(chunk) = self.warp.drain(self.lane.output_limit()) else {
                self.quantum_failed = true;
                return Some(TrackStep::StateChanged);
            };
            if let Some(chunk) = chunk {
                return Some(
                    apply_effects(&mut self.effects, chunk)
                        .and_then(|output| self.fetch(output))
                        .map_or(TrackStep::StateChanged, TrackStep::Produced),
                );
            }
            self.drain_state = DrainState::Effects;
        }

        let DrainState::Effects = self.drain_state else {
            return None;
        };
        Some(match self.drain.step(&mut self.effects) {
            EffectDrainStep::Produced(chunk) => {
                let source_end = chunk.meta.source_span.map(|span| {
                    SourceEnd::new(span.end(), span.sample_rate())
                        .with_mapping_revision(span.mapping_revision())
                });
                self.emit_output(*chunk, source_end)
                    .map_or(TrackStep::StateChanged, TrackStep::Produced)
            }
            EffectDrainStep::Progress => TrackStep::StateChanged,
            EffectDrainStep::Exhausted => {
                self.drain_state = DrainState::Exhausted;
                TrackStep::Eof
            }
        })
    }

    fn fetch(&mut self, data: AudioChunk) -> Option<Fetch<AudioChunk>> {
        let Some(span) = data.meta.source_span else {
            self.quantum_failed = true;
            return None;
        };
        let source_end = SourceEnd::new(span.end(), span.sample_rate())
            .with_mapping_revision(span.mapping_revision());
        self.emit_output(data, Some(source_end))
    }

    fn emit_output(
        &mut self,
        mut output: AudioChunk,
        source_end: Option<SourceEnd>,
    ) -> Option<Fetch<AudioChunk>> {
        if output.frames() > self.lane.output_limit() {
            self.quantum_failed = true;
            return None;
        }
        if let Some(span) = output.meta.source_span {
            output.meta.timestamp = span.position_at(0)?;
            output.meta.end_timestamp = span.position_at(span.output_frames())?;
        }
        self.lane.stamp(&mut output);
        Some(match source_end {
            Some(end) => Fetch::rendered(output, end),
            None => Fetch::data(output),
        })
    }

    /// Executes the lane batches due at its cursor, then prepares the quantum
    /// that starts there, ending it at the next batch's frame.
    fn prepare_quantum(
        &mut self,
        meta: AudioChunkInfo,
        remaining: usize,
    ) -> Result<FrameCount, WarpRenderError> {
        self.warp
            .prepare_quantum(meta, remaining, self.lane.output_limit())
    }

    fn prepare_renderers(&mut self, spec: AudioSpec) {
        self.spec = spec;
        self.warp.prepare(spec);
        if self.warp.transition_pending() {
            match self.warp.prepare_engine_latency(spec) {
                Ok(_) | Err(WarpRenderError::NeedsService) => {}
                Err(_) => {
                    self.quantum_failed = true;
                    return;
                }
            }
        }
        if self.warp.transition_pending() && matches!(self.drain_state, DrainState::Open) {
            self.drain_state = DrainState::LiveWarp;
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
        (self.staged_frames() >= frames).then(|| self.render_staged(frames))
    }

    fn render_quantum(
        &mut self,
        chunk: AudioChunk,
    ) -> ControlFlow<AudioChunk, Option<Fetch<AudioChunk>>> {
        let output = self.warp.render_quantum(chunk)?;
        if self.warp.transition_pending() {
            self.drain_state = DrainState::LiveWarp;
        }
        let output = output.and_then(|chunk| apply_effects(&mut self.effects, chunk));
        ControlFlow::Continue(output.and_then(|output| self.fetch(output)))
    }

    fn render_source_quantum(&mut self, chunk: AudioChunk) -> Option<Fetch<AudioChunk>> {
        match self.render_quantum(chunk) {
            ControlFlow::Continue(output) => output,
            ControlFlow::Break(input) => {
                debug_assert!(self.retired_input.is_none());
                self.retired_input = Some(input);
                self.quantum_failed = true;
                None
            }
        }
    }

    fn render_staged(&mut self, frames: usize) -> TrackStep<AudioChunk> {
        if self.quantum_failed {
            return self.fail(TrackFailureKind::Render);
        }
        let channels = usize::from(self.spec.channels.max(1));
        let Some(samples) = frames.checked_mul(channels) else {
            self.quantum_failed = true;
            return self.fail(TrackFailureKind::Render);
        };
        let Some(staged_meta) = self.staged_meta else {
            self.quantum_failed = true;
            return self.fail(TrackFailureKind::Render);
        };
        let Some(meta) = Self::span_meta(staged_meta, 0, frames) else {
            self.quantum_failed = true;
            return self.fail(TrackFailureKind::Render);
        };
        let Some(mut input) = self.render_input.take() else {
            return TrackStep::StateChanged;
        };
        if input.len() < samples {
            self.render_input = Some(input);
            self.quantum_failed = true;
            return self.fail(TrackFailureKind::Render);
        }
        self.staged_meta = None;
        self.prepared_frames = None;
        if input.len() > samples {
            let mut prefix = self.pools.get::<f32>();
            if prefix.ensure_len(samples).is_err() {
                self.render_input = Some(input);
                self.quantum_failed = true;
                return self.fail(TrackFailureKind::Render);
            }
            prefix.copy_from_slice(&input[..samples]);
            drop(input.drain(..samples));
            self.staged_meta = Self::span_meta(staged_meta, frames, input.len() / channels);
            self.render_input = Some(input);
            input = prefix;
        }
        match self.render_quantum(AudioChunk::new(meta, input)) {
            ControlFlow::Continue(output) => {
                output.map_or(TrackStep::StateChanged, TrackStep::Produced)
            }
            ControlFlow::Break(input) => {
                self.render_input = Some(input.samples);
                self.quantum_failed = true;
                self.fail(TrackFailureKind::Render)
            }
        }
    }

    fn render_whole_pending(&mut self) -> Option<TrackStep<AudioChunk>> {
        if self.staged_frames() != 0 {
            return None;
        }
        let prepared = self.prepared_frames?;
        let pending = self.pending_input.as_ref()?;
        if pending.consumed_frames != 0 || pending.chunk.frames() != prepared {
            return None;
        }
        let pending = self.pending_input.take()?;
        self.prepared_frames = None;
        Some(
            self.render_source_quantum(pending.chunk)
                .map_or(TrackStep::StateChanged, TrackStep::Produced),
        )
    }

    fn reset_renderers(&mut self) {
        self.warp.reset();
        reset_effects(&mut self.effects);
    }

    fn sync_discontinuity(&mut self) -> bool {
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
        if !revision_changed {
            return false;
        }
        self.discard_staged_input();
        self.reset_renderers();
        self.drain.reset();
        self.drain_state = DrainState::Open;
        true
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

    fn step_track(&mut self) -> TrackStep<AudioChunk> {
        if let Some(failure) = self.terminal_failure {
            return TrackStep::Failed(failure);
        }
        match self.service_commands() {
            Ok(true) => return TrackStep::StateChanged,
            Ok(false) => {}
            Err(error) => return self.fail(error),
        }
        if self.sync_discontinuity() {
            return TrackStep::StateChanged;
        }
        if self.quantum_failed {
            return self.fail(TrackFailureKind::Render);
        }

        if matches!(self.drain_state, DrainState::Exhausted) {
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
            return self.fail(TrackFailureKind::Render);
        }

        match self.source.step_track() {
            TrackStep::Produced(Fetch::Data { data, .. }) => {
                if data.spec() == self.spec
                    && self.prepared_frames.is_none()
                    && self
                        .prepare_quantum(data.meta, data.frames())
                        .is_ok_and(|frames| frames.get() == data.frames())
                {
                    return self
                        .render_source_quantum(data)
                        .map_or(TrackStep::StateChanged, TrackStep::Produced);
                }
                self.pending_input = Some(PendingInput {
                    chunk: data,
                    consumed_frames: 0,
                });
                TrackStep::StateChanged
            }
            TrackStep::Produced(Fetch::Failure { failure }) | TrackStep::Failed(failure) => {
                self.fail(failure)
            }
            TrackStep::Produced(fetch) => TrackStep::Produced(fetch),
            TrackStep::Eof => {
                self.begin_drain();
                let frames = self.staged_frames();
                if frames == 0 {
                    TrackStep::StateChanged
                } else {
                    let Some(frames) = self.warp.prepare_terminal_quantum(frames) else {
                        self.quantum_failed = true;
                        return self.fail(TrackFailureKind::Render);
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
        }
    }

    delegate::delegate! {
        to self.source {
            fn commit_source_end(&mut self, source_end: SourceEnd);
            fn seek(&mut self, position: Duration) -> Result<SeekOutcome, AudioReadError>;
            fn set_host_sample_rate(&mut self, rate: NonZeroU32);
            fn host_sample_rate(&self) -> Option<NonZeroU32>;
            fn retire_chunk(&self, chunk: AudioChunk);
            fn finish_deferred(&mut self);
            fn warm_up(&mut self);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        num::{NonZeroU32, NonZeroUsize},
    };

    use kithara_audio::{Fetch, TrackStep, WaitingReason};
    use kithara_bufpool::PoolRegion;
    use kithara_command::{Batch, ChannelConfig, Inbox, Outcome, Rejection, Sender, When, channel};
    use kithara_effects::held_source_frames;
    use kithara_platform::sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    };
    use kithara_signal::{AudioChunkInfo, SegmentId, SourceSpan};
    #[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
    use kithara_test_fixtures::play_fixtures::half;
    #[cfg(any(
        feature = "stretch-signalsmith",
        feature = "stretch-bungee",
        feature = "stretch-glide"
    ))]
    use kithara_test_fixtures::play_fixtures::{negative_half, three_quarter};
    use kithara_test_fixtures::play_fixtures::{negative_quarter, quarter};
    #[cfg(any(
        feature = "stretch-signalsmith",
        feature = "stretch-bungee",
        feature = "stretch-glide"
    ))]
    use kithara_test_utils::bufpool::pools_with_budget;
    use kithara_test_utils::{
        bufpool::{TestPools, pools},
        kithara,
    };
    #[cfg(any(
        feature = "stretch-signalsmith",
        feature = "stretch-bungee",
        feature = "stretch-glide"
    ))]
    use kithara_warp::WarpCapabilities;
    use kithara_warp::{SpeedCurve, StretchKind};
    #[cfg(any(
        feature = "stretch-signalsmith",
        feature = "stretch-bungee",
        feature = "stretch-glide"
    ))]
    use num_traits::AsPrimitive;

    use super::*;
    use crate::{LaneCommand, LaneFrame, LaneProtocol, consts};

    #[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
    fn keylock_backends() -> impl Iterator<Item = StretchKind> {
        StretchKind::all()
            .iter()
            .copied()
            .filter(|backend| backend.capabilities().contains(WarpCapabilities::KEYLOCK))
    }

    fn flush_deferred<S>(source: &mut S)
    where
        S: AudioSource,
    {
        let _ = source.prepare_deferred();
        source.finish_deferred();
    }

    /// The inbox of a lane no player sends to.
    fn idle_inbox() -> Inbox<LaneProtocol> {
        channel::<LaneProtocol>(ChannelConfig::builder().build()).1
    }

    fn source_stage<T>(
        pools: &PoolRegion<TestPools>,
        source: T,
        effects: Vec<Box<dyn AudioEffect>>,
        spec: AudioSpec,
    ) -> WarpSource<T, TestPools>
    where
        T: AudioSource<Chunk = AudioChunk>,
    {
        source_stage_with_quantum(pools, source, effects, spec, 128)
    }

    fn source_stage_with_quantum<T>(
        pools: &PoolRegion<TestPools>,
        source: T,
        effects: Vec<Box<dyn AudioEffect>>,
        spec: AudioSpec,
        quantum_frames: usize,
    ) -> WarpSource<T, TestPools>
    where
        T: AudioSource<Chunk = AudioChunk>,
    {
        let config = kithara_warp::WarpConfig::builder()
            .render_quantum_frames(
                NonZeroUsize::new(quantum_frames).expect("test quantum is non-zero"),
            )
            .build();
        let warp = kithara_warp::Warp::new((), &config);
        let renderer = warp.renderer(spec, pools.clone());
        let drain = EffectDrain::new(effects.len(), pools)
            .unwrap_or_else(|error| panic!("test effect drain: {error}"));
        WarpSource::new(
            source,
            renderer,
            effects,
            drain,
            spec,
            pools.clone(),
            LaneSetup {
                inbox: idle_inbox(),
                preload_chunks: NonZeroUsize::new(1).expect("preload"),
                declick: consts::DEFAULT_DECLICK,
            },
        )
    }

    struct RawSource {
        head: Arc<AtomicU64>,
        chunks: VecDeque<AudioChunk>,
    }

    impl AudioSource for RawSource {
        type Chunk = AudioChunk;

        fn seek(&mut self, target: Duration) -> Result<SeekOutcome, AudioReadError> {
            Ok(SeekOutcome::Landed {
                target,
                landed_at: target,
            })
        }

        fn set_host_sample_rate(&mut self, _rate: NonZeroU32) {}

        fn host_sample_rate(&self) -> Option<NonZeroU32> {
            None
        }

        fn step_track(&mut self) -> TrackStep<AudioChunk> {
            let Some(chunk) = self.chunks.pop_front() else {
                return TrackStep::Eof;
            };
            self.head.store(
                chunk
                    .meta
                    .frame_offset
                    .saturating_add(u64::from(chunk.meta.frames)),
                Ordering::Release,
            );
            TrackStep::Produced(Fetch::data(chunk))
        }
    }

    #[cfg(any(
        feature = "stretch-identity",
        feature = "stretch-signalsmith",
        feature = "stretch-bungee",
        feature = "stretch-glide"
    ))]
    struct FailedSource {
        failure: TrackFailureKind,
        chunks: VecDeque<AudioChunk>,
    }

    #[cfg(any(
        feature = "stretch-identity",
        feature = "stretch-signalsmith",
        feature = "stretch-bungee",
        feature = "stretch-glide"
    ))]
    impl AudioSource for FailedSource {
        type Chunk = AudioChunk;

        fn set_host_sample_rate(&mut self, _rate: NonZeroU32) {}

        fn host_sample_rate(&self) -> Option<NonZeroU32> {
            None
        }

        fn seek(&mut self, position: Duration) -> Result<SeekOutcome, AudioReadError> {
            Ok(SeekOutcome::Landed {
                target: position,
                landed_at: position,
            })
        }

        fn step_track(&mut self) -> TrackStep<AudioChunk> {
            self.chunks
                .pop_front()
                .map_or(TrackStep::Failed(self.failure), |chunk| {
                    TrackStep::Produced(Fetch::data(chunk))
                })
        }
    }

    #[cfg(any(
        feature = "stretch-identity",
        feature = "stretch-signalsmith",
        feature = "stretch-bungee",
        feature = "stretch-glide"
    ))]
    #[kithara::test(native)]
    #[cfg_attr(
        feature = "stretch-identity",
        case::identity(StretchKind::Identity, false)
    )]
    #[cfg_attr(
        feature = "stretch-signalsmith",
        case::signalsmith(StretchKind::Signalsmith, true)
    )]
    #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee, true))]
    #[cfg_attr(feature = "stretch-glide", case::glide(StretchKind::Glide, true))]
    fn upstream_terminal_failure_keeps_its_classification(
        #[case] backend: StretchKind,
        #[case] staged: bool,
    ) {
        let pools = pools();
        let spec = AudioSpec::new(2, NonZeroU32::new(48_000).expect("test sample rate"));
        let failure = TrackFailureKind::RecreateFailed { offset: 91 };
        let raw = FailedSource {
            failure,
            chunks: VecDeque::new(),
        };
        let config = kithara_warp::WarpConfig::builder()
            .backend(backend)
            .speed(0.5)
            .keylock(true)
            .build();
        let renderer = kithara_warp::Warp::new((), &config).renderer(spec, pools.clone());
        let drain = EffectDrain::new(0, &pools).expect("empty effect drain");
        let mut source = WarpSource::new(
            raw,
            renderer,
            Vec::new(),
            drain,
            spec,
            pools.clone(),
            LaneSetup {
                inbox: idle_inbox(),
                preload_chunks: NonZeroUsize::MIN,
                declick: consts::DEFAULT_DECLICK,
            },
        );
        flush_deferred(&mut source);
        assert_eq!(source.warp.requires_staging(), staged);
        let step = source.step_track();
        assert!(matches!(step, TrackStep::Failed(actual) if actual == failure));
        assert!(!source.quantum_failed);
    }

    #[cfg(any(
        feature = "stretch-identity",
        feature = "stretch-signalsmith",
        feature = "stretch-bungee",
        feature = "stretch-glide"
    ))]
    #[kithara::test(native)]
    #[cfg_attr(
        feature = "stretch-identity",
        case::identity(StretchKind::Identity, false)
    )]
    #[cfg_attr(
        feature = "stretch-signalsmith",
        case::signalsmith(StretchKind::Signalsmith, true)
    )]
    #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee, true))]
    #[cfg_attr(feature = "stretch-glide", case::glide(StretchKind::Glide, true))]
    fn upstream_terminal_failure_keeps_its_classification_after_buffered_pcm(
        #[case] backend: StretchKind,
        #[case] staged: bool,
        quarter: Vec<f32>,
    ) {
        let pools = pools();
        let spec = AudioSpec::new(2, NonZeroU32::new(48_000).expect("test sample rate"));
        let failure = TrackFailureKind::RecreateFailed { offset: 91 };
        let raw = FailedSource {
            failure,
            chunks: VecDeque::from([
                chunk_with_frames(&pools, spec, 0, 4096, &quarter),
                chunk_with_frames(&pools, spec, 4096, 17, &quarter),
            ]),
        };
        let config = kithara_warp::WarpConfig::builder()
            .backend(backend)
            .speed(0.5)
            .keylock(true)
            .render_quantum_frames(NonZeroUsize::new(128).expect("test quantum"))
            .build();
        let renderer = kithara_warp::Warp::new((), &config).renderer(spec, pools.clone());
        let effects: Vec<Box<dyn AudioEffect>> = vec![Box::<BufferThenHalveFrames>::default()];
        let drain = EffectDrain::new(effects.len(), &pools).expect("buffered effect drain");
        let mut source = WarpSource::new(
            raw,
            renderer,
            effects,
            drain,
            spec,
            pools.clone(),
            LaneSetup {
                inbox: idle_inbox(),
                preload_chunks: NonZeroUsize::MIN,
                declick: consts::DEFAULT_DECLICK,
            },
        );
        let mut staged_prefix_seen = false;
        let mut produced_nonzero_pcm = false;
        let mut terminal_seen = false;
        for _ in 0..128 {
            flush_deferred(&mut source);
            assert_eq!(source.warp.requires_staging(), staged);
            let step = source.step_track();
            staged_prefix_seen |= source.pending_input.as_ref().is_some_and(|pending| {
                pending.consumed_frames > 0 && pending.consumed_frames < pending.chunk.frames()
            });
            match step {
                TrackStep::Produced(Fetch::Data { data, .. }) => {
                    assert_eq!(data.meta.segment, SegmentId::FIRST);
                    assert!(data.frames() > 0);
                    assert_eq!(data.samples.len(), data.frames() * 2);
                    assert!(data.samples.iter().all(|sample| sample.is_finite()));
                    produced_nonzero_pcm |= data.samples.iter().any(|sample| *sample != 0.0);
                }
                TrackStep::StateChanged => {}
                TrackStep::Failed(actual) => {
                    assert_eq!(actual, failure);
                    assert!(source.source.chunks.is_empty());
                    assert!(held_source_frames(&source.effects) > 0);
                    assert!(matches!(source.drain_state, DrainState::Open));
                    assert!(!source.quantum_failed);
                    terminal_seen = true;
                    break;
                }
                _ => panic!("buffered PCM must produce or preserve the upstream failure"),
            }
        }
        assert!(
            produced_nonzero_pcm,
            "the configured backend must render PCM before failure"
        );
        assert!(terminal_seen, "the upstream failure must remain terminal");
        assert_eq!(staged_prefix_seen, staged);
        for _ in 0..3 {
            flush_deferred(&mut source);
            assert!(matches!(source.step_track(), TrackStep::Failed(actual) if actual == failure));
            assert!(held_source_frames(&source.effects) > 0);
        }
    }

    #[derive(Default)]
    struct BufferThenHalveFrames {
        buffered: Option<AudioChunk>,
    }

    impl AudioEffect for BufferThenHalveFrames {
        fn held_source_frames(&self) -> u64 {
            self.buffered
                .as_ref()
                .map_or(0, |chunk| u64::from(chunk.meta.frames))
        }

        fn reset(&mut self) {
            self.buffered = None;
        }

        delegate::delegate! {
            to self.buffered {
                #[expr($.and_then(halve_frames))]
                #[call(take)]
                fn flush(&mut self) -> Option<AudioChunk>;
                #[expr($.and_then(halve_frames))]
                #[call(replace)]
                fn process(&mut self, chunk: AudioChunk) -> Option<AudioChunk>;
            }
        }
    }

    fn halve_frames(mut chunk: AudioChunk) -> Option<AudioChunk> {
        let frames = chunk.meta.frames / 2;
        let samples = usize::try_from(frames)
            .ok()?
            .checked_mul(usize::from(chunk.meta.spec.channels))?;
        chunk.samples.truncate(samples);
        chunk.meta.frames = frames;
        chunk.meta.end_timestamp = chunk
            .meta
            .spec
            .duration_for(chunk.meta.frame_offset.saturating_add(u64::from(frames)))
            .expect("fixture timestamp fits");
        Some(chunk)
    }

    struct DeferredSource {
        log: Arc<Mutex<Vec<&'static str>>>,
        spec: AudioSpec,
    }

    impl AudioSource for DeferredSource {
        type Chunk = AudioChunk;

        fn seek(&mut self, target: Duration) -> Result<SeekOutcome, AudioReadError> {
            Ok(SeekOutcome::Landed {
                target,
                landed_at: target,
            })
        }

        fn set_host_sample_rate(&mut self, _rate: NonZeroU32) {}

        fn host_sample_rate(&self) -> Option<NonZeroU32> {
            None
        }

        fn finish_deferred(&mut self) {
            self.log.lock().push("source.finish");
        }

        fn prepare_deferred(&mut self) -> Option<AudioSpec> {
            self.log.lock().push("source.prepare");
            Some(self.spec)
        }

        fn step_track(&mut self) -> TrackStep<AudioChunk> {
            TrackStep::Blocked(WaitingReason::Waiting)
        }
    }

    struct DeferredEffect {
        log: Arc<Mutex<Vec<&'static str>>>,
        serviced: Arc<Mutex<Option<AudioSpec>>>,
    }

    impl AudioEffect for DeferredEffect {
        fn flush(&mut self) -> Option<AudioChunk> {
            None
        }

        fn held_source_frames(&self) -> u64 {
            0
        }

        fn process(&mut self, chunk: AudioChunk) -> Option<AudioChunk> {
            Some(chunk)
        }

        fn reset(&mut self) {}

        fn service_deferred(&mut self, spec: AudioSpec) {
            self.log.lock().push("effect.service");
            *self.serviced.lock() = Some(spec);
        }
    }

    struct RevisionSource {
        discontinuity: Arc<Mutex<SourceDiscontinuity>>,
        chunks: VecDeque<AudioChunk>,
    }

    impl AudioSource for RevisionSource {
        type Chunk = AudioChunk;

        fn seek(&mut self, target: Duration) -> Result<SeekOutcome, AudioReadError> {
            Ok(SeekOutcome::Landed {
                target,
                landed_at: target,
            })
        }

        fn set_host_sample_rate(&mut self, _rate: NonZeroU32) {}

        fn host_sample_rate(&self) -> Option<NonZeroU32> {
            None
        }

        fn discontinuity(&self) -> Option<SourceDiscontinuity> {
            Some(*self.discontinuity.lock())
        }

        fn step_track(&mut self) -> TrackStep<AudioChunk> {
            self.chunks
                .pop_front()
                .map_or(TrackStep::Blocked(WaitingReason::Waiting), |chunk| {
                    TrackStep::Produced(Fetch::data(chunk))
                })
        }
    }

    struct ResetCounter {
        resets: Arc<AtomicU64>,
    }

    impl AudioEffect for ResetCounter {
        fn flush(&mut self) -> Option<AudioChunk> {
            None
        }

        fn held_source_frames(&self) -> u64 {
            0
        }

        fn process(&mut self, chunk: AudioChunk) -> Option<AudioChunk> {
            Some(chunk)
        }

        fn reset(&mut self) {
            self.resets.fetch_add(1, Ordering::AcqRel);
        }
    }

    struct CountingEofSource {
        discontinuity: Arc<Mutex<Option<SourceDiscontinuity>>>,
        steps: Arc<AtomicU64>,
    }

    impl AudioSource for CountingEofSource {
        type Chunk = AudioChunk;

        fn seek(&mut self, target: Duration) -> Result<SeekOutcome, AudioReadError> {
            let revision = self
                .discontinuity
                .lock()
                .map_or(0, |stamp| stamp.revision())
                + 1;
            *self.discontinuity.lock() = Some(SourceDiscontinuity::new(
                revision,
                AudioSpec::new(2, NonZeroU32::new(44_100).expect("rate")),
            ));
            Ok(SeekOutcome::Landed {
                target,
                landed_at: target,
            })
        }

        fn set_host_sample_rate(&mut self, _rate: NonZeroU32) {}

        fn host_sample_rate(&self) -> Option<NonZeroU32> {
            None
        }

        fn discontinuity(&self) -> Option<SourceDiscontinuity> {
            *self.discontinuity.lock()
        }

        fn step_track(&mut self) -> TrackStep<AudioChunk> {
            self.steps.fetch_add(1, Ordering::AcqRel);
            TrackStep::Eof
        }
    }

    struct CountingEmptyTail {
        flushes: Arc<AtomicU64>,
        resets: Arc<AtomicU64>,
    }

    impl AudioEffect for CountingEmptyTail {
        fn flush(&mut self) -> Option<AudioChunk> {
            self.flushes.fetch_add(1, Ordering::AcqRel);
            None
        }

        fn held_source_frames(&self) -> u64 {
            0
        }

        fn process(&mut self, chunk: AudioChunk) -> Option<AudioChunk> {
            Some(chunk)
        }

        fn reset(&mut self) {
            self.resets.fetch_add(1, Ordering::AcqRel);
        }
    }

    struct SeekApplyingSource {
        spec: AudioSpec,
        revision: u64,
        pending: bool,
    }

    impl AudioSource for SeekApplyingSource {
        type Chunk = AudioChunk;

        fn seek(&mut self, target: Duration) -> Result<SeekOutcome, AudioReadError> {
            self.revision = self.revision.wrapping_add(1);
            self.pending = true;
            Ok(SeekOutcome::Landed {
                target,
                landed_at: target,
            })
        }

        fn set_host_sample_rate(&mut self, _rate: NonZeroU32) {}

        fn host_sample_rate(&self) -> Option<NonZeroU32> {
            None
        }

        fn discontinuity(&self) -> Option<SourceDiscontinuity> {
            Some(SourceDiscontinuity::new(self.revision, self.spec))
        }

        fn step_track(&mut self) -> TrackStep<AudioChunk> {
            if std::mem::take(&mut self.pending) {
                TrackStep::StateChanged
            } else {
                TrackStep::Eof
            }
        }
    }

    struct ResettingTail {
        resets: Arc<AtomicU64>,
        tail: Option<AudioChunk>,
    }

    impl AudioEffect for ResettingTail {
        fn flush(&mut self) -> Option<AudioChunk> {
            self.tail.take()
        }

        fn held_source_frames(&self) -> u64 {
            self.tail
                .as_ref()
                .map_or(0, |chunk| u64::from(chunk.meta.frames))
        }

        fn process(&mut self, chunk: AudioChunk) -> Option<AudioChunk> {
            Some(chunk)
        }

        fn reset(&mut self) {
            self.tail = None;
            self.resets.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn chunk(
        pools: &PoolRegion<TestPools>,
        spec: AudioSpec,
        frame_offset: u64,
        input: &[f32],
    ) -> AudioChunk {
        const FRAMES: usize = 128;
        chunk_with_frames(
            pools,
            spec,
            frame_offset,
            u32::try_from(FRAMES).expect("fixture frames fit u32"),
            input,
        )
    }

    fn chunk_with_frames(
        pools: &PoolRegion<TestPools>,
        spec: AudioSpec,
        frame_offset: u64,
        frames: u32,
        input: &[f32],
    ) -> AudioChunk {
        let samples = usize::try_from(frames)
            .expect("fixture frames fit usize")
            .checked_mul(usize::from(spec.channels))
            .expect("fixture sample count fits usize");
        let mut buffer = pools
            .get_with_len::<f32>(samples)
            .unwrap_or_else(|error| panic!("test sample buffer: {error}"));
        buffer.copy_from_slice(&input[..samples]);
        AudioChunk::new(
            AudioChunkInfo {
                spec,
                frames,
                frame_offset,
                timestamp: spec
                    .duration_for(frame_offset)
                    .expect("fixture timestamp fits"),
                end_timestamp: spec
                    .duration_for(frame_offset.saturating_add(u64::from(frames)))
                    .expect("fixture end timestamp fits"),
                ..Default::default()
            },
            buffer,
        )
    }

    #[cfg(any(
        feature = "stretch-signalsmith",
        feature = "stretch-bungee",
        feature = "stretch-glide"
    ))]
    #[kithara::test(native)]
    #[case::q16(16)]
    #[case::q32(32)]
    fn unity_source_chunks_bypass_staging_without_losing_terminal_input(
        #[case] quantum_frames: usize,
        quarter: Vec<f32>,
        negative_half: Vec<f32>,
        three_quarter: Vec<f32>,
    ) {
        let spec = AudioSpec::new(2, NonZeroU32::new(48_000).expect("test sample rate"));
        let pools = pools();
        let input_frames = [20_usize, 20, 10];
        let chunks = [
            chunk_with_frames(&pools, spec, 0, 20, &quarter),
            chunk_with_frames(&pools, spec, 20, 20, &negative_half),
            chunk_with_frames(&pools, spec, 40, 10, &three_quarter),
        ];
        let expected_samples = chunks
            .iter()
            .flat_map(|chunk| chunk.samples.iter().copied())
            .collect::<Vec<_>>();
        let source = RawSource {
            chunks: VecDeque::from(chunks),
            head: Arc::new(AtomicU64::new(0)),
        };
        let mut source =
            source_stage_with_quantum(&pools, source, Vec::new(), spec, quantum_frames);
        let mut output_frames = Vec::new();
        let mut output_samples = Vec::new();

        for _ in 0..32 {
            match source.step_track() {
                TrackStep::Produced(Fetch::Data { data, .. }) => {
                    output_frames.push(data.frames());
                    output_samples.extend_from_slice(&data.samples);
                }
                TrackStep::StateChanged => {}
                TrackStep::Eof => break,
                _ => panic!("staged source must only produce, progress, or finish"),
            }
            flush_deferred(&mut source);
        }

        assert!(output_frames.iter().all(|frames| *frames <= quantum_frames));
        assert_eq!(
            output_frames.iter().sum::<usize>(),
            input_frames.iter().copied().sum::<usize>()
        );
        assert_eq!(output_samples, expected_samples);
    }

    #[kithara::test]
    fn buffered_frame_changing_effect_tracks_live_and_flush_frontiers(quarter: Vec<f32>) {
        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("test sample rate"));
        let pools = pools();
        let head = Arc::new(AtomicU64::new(0));
        let source = RawSource {
            chunks: VecDeque::from([
                chunk(&pools, spec, 0, &quarter),
                chunk(&pools, spec, u64::from(128_u32), &quarter),
            ]),
            head: Arc::clone(&head),
        };
        let effects: Vec<Box<dyn AudioEffect>> = vec![Box::<BufferThenHalveFrames>::default()];
        let mut source = source_stage(&pools, source, effects, spec);

        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert_eq!(
            head.load(Ordering::Acquire),
            128,
            "one worker pass advances exactly one source transition"
        );
        flush_deferred(&mut source);
        let mut produced = None;
        for _ in 0..3 {
            match source.step_track() {
                TrackStep::Produced(fetch) => {
                    produced = Some(fetch);
                    break;
                }
                TrackStep::StateChanged => flush_deferred(&mut source),
                _ => panic!("the second raw chunk must release the first buffered span"),
            }
        }
        let Some(Fetch::Data {
            data, source_end, ..
        }) = produced
        else {
            panic!("the second raw chunk must release the first buffered span");
        };

        assert_eq!(head.load(Ordering::Acquire), 256);
        assert_eq!(data.meta.frame_offset, 0);
        assert_eq!(data.meta.frames, 64);
        assert_eq!(
            source_end,
            Some(SourceEnd::new(
                128,
                NonZeroU32::new(44_100).expect("test sample rate is non-zero"),
            )),
            "the buffered second span remains outside the live source frontier"
        );

        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        let TrackStep::Produced(Fetch::Data {
            data, source_end, ..
        }) = source.step_track()
        else {
            panic!("EOF drain must release the second buffered span");
        };

        assert_eq!(data.meta.frame_offset, 128);
        assert_eq!(data.meta.frames, 64);
        assert_eq!(
            source_end,
            Some(SourceEnd::new(
                256,
                NonZeroU32::new(44_100).expect("test sample rate is non-zero"),
            )),
            "terminal output releases the held source frontier"
        );
    }

    #[kithara::test]
    fn deferred_shell_services_effects_between_source_phases() {
        let spec = AudioSpec::new(2, NonZeroU32::new(48_000).expect("test sample rate"));
        let pools = pools();
        let log = Arc::new(Mutex::new(Vec::new()));
        let serviced = Arc::new(Mutex::new(None));
        let source = DeferredSource {
            spec,
            log: Arc::clone(&log),
        };
        let effects: Vec<Box<dyn AudioEffect>> = vec![Box::new(DeferredEffect {
            log: Arc::clone(&log),
            serviced: Arc::clone(&serviced),
        })];
        let mut source = source_stage(&pools, source, effects, spec);

        flush_deferred(&mut source);

        assert_eq!(
            log.lock().as_slice(),
            ["source.prepare", "effect.service", "source.finish"]
        );
        assert_eq!(*serviced.lock(), Some(spec));
    }

    #[kithara::test]
    fn discontinuity_refreshes_spec_without_resetting_same_revision() {
        let initial = AudioSpec::new(2, NonZeroU32::new(44_100).expect("initial rate"));
        let changed = AudioSpec::new(1, NonZeroU32::new(48_000).expect("changed rate"));
        let pools = pools();
        let discontinuity = Arc::new(Mutex::new(SourceDiscontinuity::new(7, initial)));
        let resets = Arc::new(AtomicU64::new(0));
        let source = RevisionSource {
            chunks: VecDeque::new(),
            discontinuity: Arc::clone(&discontinuity),
        };
        let effects: Vec<Box<dyn AudioEffect>> = vec![Box::new(ResetCounter {
            resets: Arc::clone(&resets),
        })];
        let mut source = source_stage(&pools, source, effects, initial);

        *discontinuity.lock() = SourceDiscontinuity::new(7, changed);
        flush_deferred(&mut source);
        assert_eq!(
            source.discontinuity().map(|stamp| *stamp.spec()),
            Some(changed)
        );
        assert_eq!(resets.load(Ordering::Acquire), 0);

        *discontinuity.lock() = SourceDiscontinuity::new(8, changed);
        flush_deferred(&mut source);
        assert_eq!(resets.load(Ordering::Acquire), 1);
    }

    #[kithara::test]
    fn unity_warp_preserves_samples_and_meta_across_discontinuity(
        quarter: Vec<f32>,
        negative_quarter: Vec<f32>,
    ) {
        let initial = AudioSpec::new(2, NonZeroU32::new(44_100).expect("initial rate"));
        let changed = AudioSpec::new(1, NonZeroU32::new(48_000).expect("changed rate"));
        let pools = pools();
        let first = chunk(&pools, initial, 256, &quarter);
        let mut first_meta = first.meta;
        first_meta.source_span = SourceSpan::new(256, 384, initial.sample_rate, 128);
        let first_samples = first.samples.to_vec();
        let mut second = chunk(&pools, changed, 512, &negative_quarter);
        second.meta.segment_index = Some(3);
        second.meta.variant_index = Some(2);
        second.meta.segment = SegmentId::FIRST.next();
        second.meta.source_byte_offset = Some(4096);
        second.meta.source_bytes = 1024;
        let mut second_meta = second.meta;
        second_meta.segment = SegmentId::FIRST;
        second_meta.lane_frame = 128;
        second_meta.source_span = SourceSpan::new(512, 640, changed.sample_rate, 128);
        let second_samples = second.samples.to_vec();
        let discontinuity = Arc::new(Mutex::new(SourceDiscontinuity::new(7, initial)));
        let source = RevisionSource {
            chunks: VecDeque::from([first, second]),
            discontinuity: Arc::clone(&discontinuity),
        };
        let effects = Vec::new();
        let mut source = source_stage(&pools, source, effects, initial);

        let TrackStep::Produced(Fetch::Data { data, .. }) = source.step_track() else {
            panic!("initial unity span must pass through");
        };
        assert_eq!(data.meta, first_meta);
        assert_eq!(&data.samples[..], &first_samples);

        *discontinuity.lock() = SourceDiscontinuity::new(8, changed);
        flush_deferred(&mut source);
        let TrackStep::Produced(Fetch::Data { data, .. }) = source.step_track() else {
            panic!("post-discontinuity unity span must pass through");
        };
        assert_eq!(data.meta, second_meta);
        assert_eq!(&data.samples[..], &second_samples);
    }

    #[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
    #[kithara::test]
    fn live_warp_drain_holds_the_source_and_seek_discards_stale_unity(
        half: Vec<f32>,
        quarter: Vec<f32>,
        three_quarter: Vec<f32>,
    ) {
        fn feed_whole_chunk(
            source: &mut WarpSource<RawSource, TestPools>,
        ) -> TrackStep<AudioChunk> {
            let TrackStep::Produced(Fetch::Data { data, .. }) = source.source.step_track() else {
                panic!("fixture provides a whole decoded chunk");
            };
            source.warp.prepare(source.spec);
            let output = source
                .warp
                .render(data)
                .continue_value()
                .expect("whole source span");
            if source.warp.transition_pending() {
                source.drain_state = DrainState::LiveWarp;
            }
            output
                .and_then(|data| source.emit_output(data, None))
                .map_or(TrackStep::StateChanged, TrackStep::Produced)
        }

        for backend in keylock_backends() {
            const ACTIVE_FRAMES: u32 = 4096;
            const UNITY_FRAMES: u32 = 4096;
            const SENTINEL_FRAMES: u32 = 4096;

            let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("test sample rate"));
            let pools = pools();
            let first_active_end = u64::from(ACTIVE_FRAMES);
            let first_unity_end = first_active_end.saturating_add(u64::from(UNITY_FRAMES));
            let sentinel_end = first_unity_end.saturating_add(u64::from(SENTINEL_FRAMES));

            let first_active = chunk_with_frames(&pools, spec, 0, ACTIVE_FRAMES, &quarter);
            let first_unity =
                chunk_with_frames(&pools, spec, first_active_end, UNITY_FRAMES, &half);
            let sentinel = chunk_with_frames(
                &pools,
                spec,
                first_unity_end,
                SENTINEL_FRAMES,
                &three_quarter,
            );
            let sentinel_ptr = sentinel.samples.as_ptr();
            let sentinel_samples = sentinel.samples.to_vec();

            let head = Arc::new(AtomicU64::new(0));
            let raw = RawSource {
                chunks: VecDeque::from([first_active, first_unity, sentinel]),
                head: Arc::clone(&head),
            };
            let render_quantum_frames = usize::try_from(ACTIVE_FRAMES)
                .expect("test quantum fits usize")
                .saturating_mul(2)
                .saturating_add(1);
            let config = kithara_warp::WarpConfig::builder()
                .speed(0.5)
                .keylock(true)
                .backend(backend)
                .render_quantum_frames(
                    NonZeroUsize::new(render_quantum_frames).expect("test quantum is non-zero"),
                )
                .build();
            let renderer = kithara_warp::Warp::new((), &config).renderer(spec, pools.clone());
            let effects = Vec::new();
            let drain = EffectDrain::new(effects.len(), &pools)
                .unwrap_or_else(|error| panic!("test effect drain: {error}"));
            let (mut lane, inbox) = channel::<LaneProtocol>(ChannelConfig::builder().build());
            let mut source = WarpSource::new(
                raw,
                renderer,
                effects,
                drain,
                spec,
                pools.clone(),
                LaneSetup {
                    inbox,
                    preload_chunks: NonZeroUsize::new(1).expect("preload"),
                    declick: consts::DEFAULT_DECLICK,
                },
            );

            let initial = feed_whole_chunk(&mut source);
            assert!(matches!(
                &initial,
                TrackStep::Produced(_) | TrackStep::StateChanged
            ));
            assert_eq!(head.load(Ordering::Acquire), first_active_end);
            flush_deferred(&mut source);
            if matches!(&initial, TrackStep::StateChanged) {
                let TrackStep::Produced(_) = source.step_track() else {
                    panic!("the first active quantum must render");
                };
                flush_deferred(&mut source);
            }

            source
                .warp
                .set_speed(SpeedCurve::Constant(1.0), 1)
                .expect("unity command");
            let transition = feed_whole_chunk(&mut source);
            assert!(matches!(
                &transition,
                TrackStep::Produced(_) | TrackStep::StateChanged
            ));
            assert_eq!(head.load(Ordering::Acquire), first_unity_end);
            flush_deferred(&mut source);
            if matches!(&transition, TrackStep::StateChanged) {
                let TrackStep::Produced(_) = source.step_track() else {
                    panic!("active-to-unity transition must emit its first tail quantum");
                };
            }
            assert_eq!(head.load(Ordering::Acquire), first_unity_end);
            assert!(source.warp.transition_pending());

            lane.send(
                When::Next,
                command_batch(LaneCommand::Segment {
                    id: SegmentId::FIRST.next(),
                    from: Duration::from_secs(1),
                    speed: SpeedCurve::Constant(1.0),
                }),
            )
            .expect("segment batch");
            assert!(matches!(source.step_track(), TrackStep::StateChanged));
            assert_eq!(source.cursor().segment, SegmentId::FIRST.next());
            assert_eq!(head.load(Ordering::Acquire), first_unity_end);
            assert!(!source.warp.transition_pending());

            flush_deferred(&mut source);
            let resumed = source.step_track();
            let resumed = match resumed {
                TrackStep::Produced(fetch) => fetch,
                TrackStep::StateChanged => {
                    flush_deferred(&mut source);
                    let TrackStep::Produced(fetch) = source.step_track() else {
                        panic!("playback must resume with the post-seek source chunk");
                    };
                    fetch
                }
                _ => panic!("playback must resume with the post-seek source chunk"),
            };
            let Fetch::Data { data, .. } = resumed else {
                panic!("playback must resume with post-seek audio data");
            };
            assert_eq!(head.load(Ordering::Acquire), sentinel_end);
            assert_eq!(data.samples.as_ptr(), sentinel_ptr);
            assert_eq!(&data.samples[..], &sentinel_samples);
            assert_eq!(data.meta.segment, SegmentId::FIRST.next());
        }
    }

    #[cfg(any(
        feature = "stretch-signalsmith",
        feature = "stretch-bungee",
        feature = "stretch-glide"
    ))]
    #[kithara::test(native)]
    async fn rejected_staged_quantum_retains_both_owning_buffers(quarter: Vec<f32>) {
        const FRAMES: usize = 64;
        #[kithara::allow_block]
        fn prepare(quarter: &[f32]) -> (WarpSource<RawSource, TestPools>, *const f32, *const f32) {
            let pools = pools();
            let spec = AudioSpec::new(2, NonZeroU32::new(48_000).expect("sample rate"));
            let raw = RawSource {
                head: Arc::new(AtomicU64::new(0)),
                chunks: VecDeque::new(),
            };
            let decoded = chunk_with_frames(&pools, spec, 0, 64, quarter);
            let decoded_pointer = decoded.samples.as_ptr();
            let mut source = source_stage_with_quantum(&pools, raw, Vec::new(), spec, FRAMES);
            source.pending_input = Some(PendingInput {
                chunk: decoded,
                consumed_frames: 0,
            });
            source.prepare_staging();
            assert_eq!(source.prepared_frames, Some(FRAMES));
            assert!(source.stage_pending());
            assert!(source.pending_input.is_none());
            let staged_pointer = source.render_input.as_ref().expect("staged PCM").as_ptr();
            assert_ne!(decoded_pointer, staged_pointer);
            source.warp.reset();
            (source, decoded_pointer, staged_pointer)
        }
        #[kithara::no_block(budget_ms = 1_000)]
        async fn reject(source: &mut WarpSource<RawSource, TestPools>) -> TrackStep<AudioChunk> {
            source.render_staged(FRAMES)
        }
        #[kithara::allow_block]
        fn release(source: WarpSource<RawSource, TestPools>) {
            drop(source);
        }
        let (mut source, decoded_pointer, staged_pointer) = prepare(&quarter);
        let result = reject(&mut source).await;
        assert!(matches!(result, TrackStep::Failed(_)));
        assert!(source.quantum_failed);
        let decoded = source
            .retired_input
            .as_ref()
            .expect("decoded retirement retained");
        assert_eq!(decoded.samples.as_ptr(), decoded_pointer);
        assert_eq!(&decoded.samples[..], &quarter[..FRAMES * 2]);
        let staged = source
            .render_input
            .as_ref()
            .expect("rejected staging retained");
        assert_eq!(staged.as_ptr(), staged_pointer);
        assert_eq!(&staged[..], &quarter[..FRAMES * 2]);
        release(source);
    }

    #[kithara::test(native)]
    #[cfg(feature = "stretch-signalsmith")]
    fn projected_backend_switch_keeps_pending_input_during_resident_output(quarter: Vec<f32>) {
        use kithara_warp::{GridSegment, RegionPlan};
        let spec = AudioSpec::new(2, NonZeroU32::new(48_000).expect("sample rate"));
        let pools = pools();
        let config = kithara_warp::WarpConfig::builder()
            .speed(1.0)
            .keylock(true)
            .backend(StretchKind::Signalsmith)
            .render_quantum_frames(NonZeroUsize::new(128).expect("quantum"))
            .region_plan(Arc::new(
                RegionPlan::new(vec![GridSegment::new(0, 480_000, 1.5)]).expect("region geometry"),
            ))
            .build();
        let head = Arc::new(AtomicU64::new(0));
        let raw = RawSource {
            head: Arc::clone(&head),
            chunks: (0..8)
                .map(|index| chunk_with_frames(&pools, spec, index * 4096, 4096, &quarter))
                .collect(),
        };
        let renderer = kithara_warp::Warp::new((), &config).renderer(spec, pools.clone());
        let drain = EffectDrain::new(0, &pools).expect("empty effect drain");
        let mut source = WarpSource::new(
            raw,
            renderer,
            Vec::new(),
            drain,
            spec,
            pools.clone(),
            LaneSetup {
                inbox: idle_inbox(),
                preload_chunks: NonZeroUsize::new(1).expect("preload"),
                declick: consts::DEFAULT_DECLICK,
            },
        );
        let mut produced = 0;
        for _ in 0..128 {
            flush_deferred(&mut source);
            match source.step_track() {
                TrackStep::Produced(Fetch::Data { data, .. }) => {
                    assert!(data.frames() > 0);
                    produced += 1;
                }
                TrackStep::StateChanged | TrackStep::Blocked(_) => {}
                _ => panic!("projected source must stay live before the switch"),
            }
            if produced >= 4 && source.pending_input.is_some() && source.prepared_frames.is_none() {
                break;
            }
        }
        assert!(
            produced >= 4,
            "the old backend must emit before it is replaced"
        );
        let pending = source.pending_input.as_ref().expect("held decoded input");
        let pointer = pending.chunk.samples.as_ptr();
        let meta = pending.chunk.meta;
        let consumed = pending.consumed_frames;
        let input_head = head.load(Ordering::Acquire);
        source.warp.set_keylock(false);
        source.warp.prepare(spec);
        assert!(
            source.warp.transition_pending(),
            "the old backend needs retirement"
        );
        source.prepare_staging();
        assert!(
            !source.quantum_failed,
            "NeedsService is not a fatal render error"
        );
        assert!(
            matches!(source.drain_state, DrainState::LiveWarp),
            "backend retirement must be serviced before retrying the held input"
        );
        let mut resident_output = false;
        let mut resumed_input = false;
        for _ in 0..128 {
            flush_deferred(&mut source);
            let before = source
                .pending_input
                .as_ref()
                .expect("input remains held until resumed")
                .consumed_frames;
            let step = source.step_track();
            assert!(
                !matches!(step, TrackStep::Failed(_) | TrackStep::Eof),
                "backend service must neither fail nor finish the source"
            );
            let pending = source
                .pending_input
                .as_ref()
                .expect("one resumed quantum leaves decoded input");
            assert_eq!(pending.chunk.samples.as_ptr(), pointer);
            assert_eq!(pending.chunk.meta, meta);
            assert_eq!(
                &pending.chunk.samples[..],
                &quarter[..pending.chunk.samples.len()]
            );
            assert_eq!(head.load(Ordering::Acquire), input_head);
            if let TrackStep::Produced(Fetch::Data { data, .. }) = step {
                assert!(data.frames() > 0);
                if pending.consumed_frames == before {
                    resident_output = true;
                    assert_eq!(pending.consumed_frames, consumed);
                }
            }
            if pending.consumed_frames > consumed {
                resumed_input = true;
                break;
            }
        }
        assert!(
            resident_output,
            "buffered projection must emit without taking new decoded frames"
        );
        assert!(
            resumed_input,
            "the replacement backend must resume the retained input"
        );
        assert!(!source.quantum_failed);
    }

    #[cfg(any(
        feature = "stretch-signalsmith",
        feature = "stretch-bungee",
        feature = "stretch-glide"
    ))]
    #[kithara::test]
    fn unavailable_warp_target_fails_before_pulling_source(quarter: Vec<f32>) {
        for backend in StretchKind::all()
            .iter()
            .copied()
            .filter(|backend| backend.capabilities().contains(WarpCapabilities::RATE))
        {
            let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("test sample rate"));
            let source_pools = pools();
            let head = Arc::new(AtomicU64::new(0));
            let raw = RawSource {
                chunks: VecDeque::from([chunk(&source_pools, spec, 0, &quarter)]),
                head: Arc::clone(&head),
            };
            let config = kithara_warp::WarpConfig::builder()
                .speed(0.5)
                .keylock(true)
                .backend(backend)
                .build();
            let target_pools = pools_with_budget(0);
            let renderer =
                kithara_warp::Warp::new((), &config).renderer(spec, target_pools.clone());
            let effects = Vec::new();
            let drain = EffectDrain::new(effects.len(), &target_pools)
                .unwrap_or_else(|error| panic!("test effect drain: {error}"));
            let mut source = WarpSource::new(
                raw,
                renderer,
                effects,
                drain,
                spec,
                target_pools.clone(),
                LaneSetup {
                    inbox: idle_inbox(),
                    preload_chunks: NonZeroUsize::new(1).expect("preload"),
                    declick: consts::DEFAULT_DECLICK,
                },
            );

            for _ in 0..3 {
                flush_deferred(&mut source);
                assert!(matches!(source.step_track(), TrackStep::Failed(_)));
                assert_eq!(head.load(Ordering::Acquire), 0);
            }
        }
    }

    #[kithara::test]
    fn seek_cancels_stale_tail_and_resets_effects_once(quarter: Vec<f32>) {
        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("test sample rate"));
        let pools = pools();
        let resets = Arc::new(AtomicU64::new(0));
        let source = SeekApplyingSource {
            spec,
            revision: 0,
            pending: false,
        };
        let effects: Vec<Box<dyn AudioEffect>> = vec![Box::new(ResettingTail {
            resets: Arc::clone(&resets),
            tail: Some(chunk(&pools, spec, 128, &quarter)),
        })];
        let mut source = source_stage(&pools, source, effects, spec);

        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert!(matches!(
            source.source.seek(Duration::from_secs(1)),
            Ok(SeekOutcome::Landed { .. })
        ));
        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert_eq!(
            resets.load(Ordering::Acquire),
            1,
            "stale seek drain resets renderers before the source adopts the epoch"
        );
        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert_eq!(resets.load(Ordering::Acquire), 1);

        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert!(matches!(source.step_track(), TrackStep::Eof));
    }

    #[kithara::test]
    fn every_effect_tail_precedes_the_single_eof(quarter: Vec<f32>) {
        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("test sample rate"));
        let pools = pools();
        let source = RawSource {
            chunks: VecDeque::new(),
            head: Arc::new(AtomicU64::new(0)),
        };
        let effects: Vec<Box<dyn AudioEffect>> = vec![
            Box::new(ResettingTail {
                resets: Arc::new(AtomicU64::new(0)),
                tail: Some(chunk(&pools, spec, 128, &quarter)),
            }),
            Box::new(ResettingTail {
                resets: Arc::new(AtomicU64::new(0)),
                tail: Some(chunk(&pools, spec, 256, &quarter)),
            }),
        ];
        let mut source = source_stage(&pools, source, effects, spec);

        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        for expected in [128, 256] {
            let TrackStep::Produced(Fetch::Data {
                data, source_end, ..
            }) = source.step_track()
            else {
                panic!("effect tail must be emitted before EOF");
            };
            assert_eq!(data.meta.frame_offset, expected);
            assert_eq!(source_end, None, "effect-only tails do not advance source");
        }
        assert!(matches!(source.step_track(), TrackStep::Eof));
    }

    #[kithara::test]
    fn exhausted_drain_stays_terminal_for_the_decode_epoch() {
        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("test sample rate"));
        let pools = pools();
        let steps = Arc::new(AtomicU64::new(0));
        let flushes = Arc::new(AtomicU64::new(0));
        let resets = Arc::new(AtomicU64::new(0));
        let discontinuity = Arc::new(Mutex::new(None));
        let source = CountingEofSource {
            discontinuity: Arc::clone(&discontinuity),
            steps: Arc::clone(&steps),
        };
        let effects: Vec<Box<dyn AudioEffect>> = vec![Box::new(CountingEmptyTail {
            flushes: Arc::clone(&flushes),
            resets: Arc::clone(&resets),
        })];
        let mut source = source_stage(&pools, source, effects, spec);

        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert!(matches!(source.step_track(), TrackStep::Eof));
        for _ in 0..3 {
            assert!(matches!(source.step_track(), TrackStep::Eof));
        }

        assert_eq!(steps.load(Ordering::Acquire), 1);
        assert_eq!(flushes.load(Ordering::Acquire), 1);

        assert!(matches!(
            source.source.seek(Duration::from_secs(1)),
            Ok(SeekOutcome::Landed { .. })
        ));
        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert!(matches!(source.step_track(), TrackStep::Eof));
        assert!(matches!(source.step_track(), TrackStep::Eof));
        assert_eq!(steps.load(Ordering::Acquire), 2);
        assert_eq!(flushes.load(Ordering::Acquire), 2);
        assert_eq!(resets.load(Ordering::Acquire), 1);

        *discontinuity.lock() = Some(SourceDiscontinuity::new(1, spec));
        assert!(matches!(source.step_track(), TrackStep::Eof));
        assert!(matches!(source.step_track(), TrackStep::Eof));
        assert_eq!(steps.load(Ordering::Acquire), 2);
        assert_eq!(flushes.load(Ordering::Acquire), 2);
        assert_eq!(resets.load(Ordering::Acquire), 1);

        *discontinuity.lock() = Some(SourceDiscontinuity::new(2, spec));
        flush_deferred(&mut source);
        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert!(matches!(source.step_track(), TrackStep::Eof));
        assert!(matches!(source.step_track(), TrackStep::Eof));
        assert_eq!(steps.load(Ordering::Acquire), 3);
        assert_eq!(flushes.load(Ordering::Acquire), 3);
        assert_eq!(resets.load(Ordering::Acquire), 2);
    }

    /// One emitted chunk on the lane axis and the source span it renders.
    struct Emitted {
        lane_start: u64,
        source_start: u64,
        revision: u64,
        samples: Vec<f32>,
    }

    fn speed_batch(speed: f32) -> Batch<LaneProtocol> {
        command_batch(LaneCommand::SetSpeed(SpeedCurve::Constant(speed)))
    }

    fn command_batch(command: LaneCommand) -> Batch<LaneProtocol> {
        Batch {
            basis: Vec::new(),
            commands: vec![command],
        }
    }

    /// A lane over constant source audio at unity speed, with the player end
    /// of its channel.
    fn speed_lane(
        pools: &PoolRegion<TestPools>,
        quarter: &[f32],
    ) -> (WarpSource<RawSource, TestPools>, Sender<LaneProtocol>) {
        lane_over(pools, 3, |_| quarter, 1.0, (StretchKind::default(), false))
    }

    /// A lane over `signal` at unity speed, rendered by `backend` with keylock
    /// when `keylock`.
    #[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
    fn stretch_lane(
        pools: &PoolRegion<TestPools>,
        (backend, keylock): (StretchKind, bool),
        signal: &[f32],
    ) -> (WarpSource<RawSource, TestPools>, Sender<LaneProtocol>) {
        let chunks = signal.len() / (2 * consts::LANE_CHUNK_FRAMES as usize);
        lane_over(
            pools,
            u32::try_from(chunks).expect("test chunk count fits u32"),
            |index| &signal[index as usize * 2 * consts::LANE_CHUNK_FRAMES as usize..],
            1.0,
            (backend, keylock),
        )
    }

    /// A lane over `chunks` source chunks of [`consts::LANE_CHUNK_FRAMES`] frames, the
    /// `index`th copied from the front of `samples(index)`, starting at `speed` on
    /// `backend`, with keylock when `keylock`.
    fn lane_over<'a>(
        pools: &PoolRegion<TestPools>,
        chunks: u32,
        samples: impl Fn(u32) -> &'a [f32],
        speed: f32,
        (backend, keylock): (StretchKind, bool),
    ) -> (WarpSource<RawSource, TestPools>, Sender<LaneProtocol>) {
        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("test sample rate"));
        let chunks = (0..chunks)
            .map(|index| {
                chunk_with_frames(
                    pools,
                    spec,
                    u64::from(index * consts::LANE_CHUNK_FRAMES),
                    consts::LANE_CHUNK_FRAMES,
                    samples(index),
                )
            })
            .collect();
        let raw = RawSource {
            chunks,
            head: Arc::new(AtomicU64::new(0)),
        };
        let (lane, inbox) = channel::<LaneProtocol>(ChannelConfig::builder().build());
        let config = kithara_warp::WarpConfig::builder()
            .speed(speed)
            .keylock(keylock)
            .backend(backend)
            .render_quantum_frames(NonZeroUsize::new(256).expect("test quantum is non-zero"))
            .build();
        let renderer = kithara_warp::Warp::new((), &config).renderer(spec, pools.clone());
        let drain =
            EffectDrain::new(0, pools).unwrap_or_else(|error| panic!("test effect drain: {error}"));
        let source = WarpSource::new(
            raw,
            renderer,
            Vec::new(),
            drain,
            spec,
            pools.clone(),
            LaneSetup {
                inbox,
                preload_chunks: NonZeroUsize::new(1).expect("preload"),
                declick: consts::DEFAULT_DECLICK,
            },
        );
        (source, lane)
    }

    /// Steps the lane from output frame `from` until it emitted `until`.
    fn emit(source: &mut WarpSource<RawSource, TestPools>, from: u64, until: u64) -> Vec<Emitted> {
        let mut emitted = Vec::new();
        let mut cursor = from;
        for _ in 0..4096 {
            if cursor >= until {
                return emitted;
            }
            match source.step_track() {
                TrackStep::Produced(Fetch::Data { data, .. }) => {
                    emitted.push(Emitted {
                        lane_start: cursor,
                        source_start: data.meta.frame_offset,
                        revision: data.meta.render_revision,
                        samples: data.samples.to_vec(),
                    });
                    cursor += u64::from(data.meta.frames);
                }
                TrackStep::StateChanged | TrackStep::Blocked(_) => {}
                TrackStep::Eof => panic!("the lane ended at frame {cursor} before {until}"),
                _ => panic!("the lane failed at frame {cursor} before {until}"),
            }
            flush_deferred(source);
        }
        panic!("the lane stalled at frame {cursor} before {until}");
    }

    #[kithara::test]
    fn a_speed_batch_applies_on_its_lane_frame(quarter: Vec<f32>) {
        const AT: u64 = 1_000;
        let pools = pools();
        let (mut source, mut lane) = speed_lane(&pools, &quarter);
        let seq = lane
            .send(
                When::At(LaneFrame {
                    segment: SegmentId::FIRST,
                    frame: AT,
                }),
                speed_batch(1.25),
            )
            .expect("the lane has room for one batch");

        let emitted = emit(&mut source, 0, AT + 2_000);

        if StretchKind::default().capabilities().is_empty() {
            for chunk in &emitted {
                assert_eq!(chunk.source_start, chunk.lane_start);
                assert_eq!(&chunk.samples[..], &quarter[..chunk.samples.len()]);
            }
        }
        let boundary = emitted
            .iter()
            .position(|chunk| chunk.lane_start == AT)
            .expect("a quantum starts on the batch's frame");
        assert!(
            emitted[..boundary].iter().all(|chunk| chunk.revision == 0),
            "frames before the batch render under the initial speed"
        );
        assert!(
            emitted[boundary..]
                .iter()
                .all(|chunk| chunk.revision == seq.get()),
            "frames from the batch on render under it"
        );
        let outcomes = lane
            .receipts()
            .map(|receipt| {
                let seq = receipt.seq();
                let (outcome, _) = receipt.into();
                (seq, outcome)
            })
            .collect::<Vec<(_, Outcome<LaneProtocol>)>>();
        assert!(
            matches!(
                outcomes.as_slice(),
                [(applied, Outcome::Applied { at: LaneFrame { segment: SegmentId::FIRST, frame: AT }, .. })] if *applied == seq
            ),
            "the receipt names the batch's frame: {outcomes:?}"
        );
    }

    #[kithara::test]
    fn a_batch_for_a_rendered_lane_frame_comes_back_late(quarter: Vec<f32>) {
        let pools = pools();
        let (mut source, mut lane) = speed_lane(&pools, &quarter);
        let _ = emit(&mut source, 0, 1_000);

        lane.send(
            When::At(LaneFrame {
                segment: SegmentId::FIRST,
                frame: 500,
            }),
            speed_batch(1.25),
        )
        .expect("the lane has room for one batch");
        let emitted = emit(&mut source, 1_000, 2_000);

        assert!(
            emitted.iter().all(|chunk| chunk.revision == 0),
            "a late batch renders nothing"
        );
        let outcomes = lane
            .receipts()
            .map(|receipt| <(Outcome<LaneProtocol>, _)>::from(receipt).0)
            .collect::<Vec<_>>();
        assert!(
            matches!(outcomes.as_slice(), [Outcome::Rejected(Rejection::Late)]),
            "a rendered frame answers late: {outcomes:?}"
        );
    }

    #[cfg(any(
        feature = "stretch-signalsmith",
        feature = "stretch-bungee",
        feature = "stretch-glide"
    ))]
    #[kithara::test]
    fn a_speed_change_continues_the_source_at_the_new_step(quarter: Vec<f32>) {
        const AT: u64 = 1_000;
        const SPEED: f64 = 1.25;
        let pools = pools();
        let (mut source, mut lane) = speed_lane(&pools, &quarter);
        lane.send(
            When::At(LaneFrame {
                segment: SegmentId::FIRST,
                frame: AT,
            }),
            speed_batch(1.25),
        )
        .expect("the lane has room for one batch");

        let emitted = emit(&mut source, 0, AT + 4_000);

        for chunk in &emitted {
            let expected = if chunk.lane_start < AT {
                chunk.lane_start
            } else {
                let elapsed: f64 = (chunk.lane_start - AT).as_();
                let stretched: u64 = (elapsed * SPEED).round().as_();
                AT + stretched
            };
            assert!(
                chunk.source_start.abs_diff(expected) <= 1,
                "lane frame {} renders source frame {}, not {expected}",
                chunk.lane_start,
                chunk.source_start
            );
        }
    }

    /// A quantum at a speed other than unity cannot end on an arbitrary frame
    /// by rounding whole source frames; the lane still lands it on the
    /// batch's frame, so the batch applies there.
    #[cfg(any(
        feature = "stretch-signalsmith",
        feature = "stretch-bungee",
        feature = "stretch-glide"
    ))]
    #[kithara::test]
    fn a_speed_batch_lands_on_its_frame_from_any_speed(quarter: Vec<f32>) {
        const AT: u64 = 1_000;
        let pools = pools();
        let (mut source, mut lane) = lane_over(
            &pools,
            3,
            |_| &quarter,
            0.8,
            (StretchKind::default(), false),
        );
        lane.send(
            When::At(LaneFrame {
                segment: SegmentId::FIRST,
                frame: AT,
            }),
            speed_batch(1.25),
        )
        .expect("the lane has room for one batch");

        let emitted = emit(&mut source, 0, AT + 1_000);

        assert!(
            emitted.iter().any(|chunk| chunk.lane_start == AT),
            "a quantum starts on the batch's frame: {:?}",
            emitted
                .iter()
                .map(|chunk| chunk.lane_start)
                .collect::<Vec<_>>()
        );
    }

    /// A stereo linear chirp, 220 Hz to 1760 Hz: every source frame sounds a
    /// frequency of its own, so audio rendered from another frame decorrelates.
    #[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
    fn chirp(frames: usize) -> Vec<f32> {
        let rate = 44_100.0_f64;
        let length: f64 = frames.as_();
        let span = length / rate;
        (0..frames)
            .flat_map(|frame| {
                let frame: f64 = frame.as_();
                let time = frame / rate;
                let phase = std::f64::consts::TAU
                    * time.mul_add(220.0, (1_760.0 - 220.0) / (2.0 * span) * time * time);
                let sample: f32 = (0.5 * phase.sin()).as_();
                [sample, sample]
            })
            .collect()
    }

    /// A lane frame as an index into [`lane_pcm`].
    #[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
    fn pcm_index(frame: u64) -> usize {
        usize::try_from(frame).expect("test lane frame fits usize")
    }

    /// The left channel the lane emitted, indexed by lane frame.
    #[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
    fn lane_pcm(emitted: &[Emitted]) -> Vec<f32> {
        emitted
            .iter()
            .flat_map(|chunk| chunk.samples.iter().step_by(2).copied())
            .collect()
    }

    /// The offset of `rendered` within `reference` around `center` that
    /// correlates best, within `reach` frames, and that correlation.
    #[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
    fn alignment(rendered: &[f32], reference: &[f32], center: usize, reach: usize) -> (i64, f64) {
        let energy = |samples: &[f32]| {
            samples
                .iter()
                .map(|&sample| f64::from(sample) * f64::from(sample))
                .sum::<f64>()
        };
        let rendered_energy = energy(rendered);
        (0..=2 * reach)
            .map(|shift| {
                let window = &reference[center - reach + shift..][..rendered.len()];
                let product = rendered
                    .iter()
                    .zip(window)
                    .map(|(&left, &right)| f64::from(left) * f64::from(right))
                    .sum::<f64>();
                let correlation = product / (rendered_energy * energy(window)).sqrt();
                (shift as i64 - reach as i64, correlation)
            })
            .fold(
                (0, f64::MIN),
                |best, next| {
                    if next.1 > best.1 { next } else { best }
                },
            )
    }

    /// A stereo 440 Hz sine at half scale: a keylock engine keeps its pitch at
    /// any speed, so its output moves between two samples by at most the
    /// sine's own step.
    #[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
    fn sine(frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|frame| {
                let frame: f64 = frame.as_();
                let phase = std::f64::consts::TAU * 440.0 * frame / 44_100.0;
                let sample: f32 = (0.5 * phase.sin()).as_();
                [sample, sample]
            })
            .collect()
    }

    /// A keylock speed change that lands while the previous change's tail
    /// still fades out fades from what sounds on its frame, so the output
    /// never jumps: no step between two samples exceeds three times the
    /// sine's largest step.
    #[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
    #[kithara::test]
    fn a_speed_change_within_a_tail_fade_fades_from_what_sounds() {
        for backend in keylock_backends() {
            const ENGAGE: u64 = 1_024;
            const FIRST: u64 = 4_099;
            const SECOND: u64 = FIRST + 512;
            const SETTLE: u64 = 16_384;
            let largest_step: f32 = (std::f64::consts::TAU * 440.0 / 44_100.0 * 0.5).as_();
            let signal = sine(12 * consts::LANE_CHUNK_FRAMES as usize);
            let pools = pools();
            let (mut source, mut lane) = stretch_lane(&pools, (backend, true), &signal);
            for (at, speed) in [(ENGAGE, 0.8), (FIRST, 1.25), (SECOND, 0.94)] {
                lane.send(
                    When::At(LaneFrame {
                        segment: SegmentId::FIRST,
                        frame: at,
                    }),
                    speed_batch(speed),
                )
                .expect("the lane has room for each change");
            }

            let pcm = lane_pcm(&emit(&mut source, 0, SECOND + SETTLE));

            let (frame, step) = pcm[pcm_index(FIRST) - 256..pcm_index(SECOND + SETTLE)]
                .windows(2)
                .map(|pair| (pair[1] - pair[0]).abs())
                .enumerate()
                .fold((0, 0.0_f32), |worst, (offset, step)| {
                    if step > worst.1 {
                        (offset, step)
                    } else {
                        worst
                    }
                });
            assert!(
                step <= 3.0 * largest_step,
                "{backend:?}: the output jumps {step:.3} at lane frame {} (the sine steps at most \
                 {largest_step:.3}); changes at {FIRST} and {SECOND}",
                pcm_index(FIRST) - 256 + frame,
            );
        }
    }

    /// A keylock engine that changes speed on lane frame X renders from X on
    /// what an engine started at X's source frame with the new speed renders:
    /// the change lands on its frame and the source does not jump. The old
    /// engine's tail fades out within `SETTLE` frames.
    #[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
    #[kithara::test]
    fn a_keylock_speed_change_renders_on_as_an_engine_started_on_its_frame() {
        for backend in keylock_backends() {
            const ENGAGE: u64 = 1_024;
            /// At 0.8 the 3075 frames from ENGAGE play 2460 source frames whole.
            const AT: u64 = 4_099;
            const SETTLE: u64 = 16_384;
            const WINDOW: usize = 4_096;
            const REACH: usize = 2_048;
            let signal = chirp(12 * consts::LANE_CHUNK_FRAMES as usize);
            let pools = pools();
            let (mut changed, mut lane) = stretch_lane(&pools, (backend, true), &signal);
            lane.send(
                When::At(LaneFrame {
                    segment: SegmentId::FIRST,
                    frame: ENGAGE,
                }),
                speed_batch(0.8),
            )
            .expect("the lane has room for the engaging batch");
            lane.send(
                When::At(LaneFrame {
                    segment: SegmentId::FIRST,
                    frame: AT,
                }),
                speed_batch(1.25),
            )
            .expect("the lane has room for the change");
            let emitted = emit(&mut changed, 0, AT + SETTLE + WINDOW as u64);
            let cue = ENGAGE + (AT - ENGAGE) * 4 / 5;
            assert_eq!(
                emitted
                    .iter()
                    .find(|chunk| chunk.lane_start == AT)
                    .map(|chunk| chunk.source_start),
                Some(cue),
                "the speed change starts on its exact lane and source frame",
            );

            let (mut started, mut fresh) = stretch_lane(&pools, (backend, false), &signal);
            fresh
                .send(
                    When::At(LaneFrame {
                        segment: SegmentId::FIRST,
                        frame: ENGAGE,
                    }),
                    speed_batch(0.8),
                )
                .expect("the lane has room for the reference history");
            let mut start = speed_batch(1.25);
            start.commands.push(LaneCommand::SetKeylock(true));
            fresh
                .send(
                    When::At(LaneFrame {
                        segment: SegmentId::FIRST,
                        frame: AT,
                    }),
                    start,
                )
                .expect("the lane has room for the start");
            let reference = emit(&mut started, 0, AT + SETTLE + (WINDOW + REACH) as u64);
            assert_eq!(
                reference
                    .iter()
                    .find(|chunk| chunk.lane_start == AT)
                    .map(|chunk| chunk.source_start),
                Some(cue),
                "the fresh engine starts on the same lane and source frame",
            );

            let rendered = &lane_pcm(&emitted)[pcm_index(AT + SETTLE)..][..WINDOW];
            let (offset, correlation) = alignment(
                rendered,
                &lane_pcm(&reference),
                pcm_index(AT + SETTLE),
                REACH,
            );
            assert!(
                offset == 0 && correlation > 0.95,
                "{backend:?}: after the change at lane frame {AT} (source {cue}) the lane \
                 renders {offset} frames off a fresh engine, correlation {correlation:.3}"
            );
        }
    }

    /// A batch that changes the engine on lane frame X renders from X on what
    /// the new engine started at X's source frame renders: the engine changes
    /// on its frame, the source does not jump, and the receipt names X. The
    /// old engine's tail fades out within `SETTLE` frames.
    #[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
    #[kithara::test]
    fn an_engine_batch_renders_on_as_its_engine_started_on_its_frame() {
        const ENGAGE: u64 = 1_024;
        /// At 1.25 the 3072 frames from ENGAGE play 3840 source frames whole.
        const AT: u64 = 4_096;
        const SETTLE: u64 = 16_384;
        const WINDOW: usize = 4_096;
        const REACH: usize = 2_048;
        for backend in keylock_backends() {
            for next in keylock_backends() {
                let (from, command) = if backend == next {
                    ((backend, false), LaneCommand::SetKeylock(true))
                } else {
                    ((backend, true), LaneCommand::SetBackend(next))
                };
                let to = (next, true);
                let signal = chirp(12 * consts::LANE_CHUNK_FRAMES as usize);
                let pools = pools();
                let (mut changed, mut lane) = stretch_lane(&pools, from, &signal);
                lane.send(
                    When::At(LaneFrame {
                        segment: SegmentId::FIRST,
                        frame: ENGAGE,
                    }),
                    speed_batch(1.25),
                )
                .expect("the lane has room for the engaging batch");
                let seq = lane
                    .send(
                        When::At(LaneFrame {
                            segment: SegmentId::FIRST,
                            frame: AT,
                        }),
                        command_batch(command.clone()),
                    )
                    .expect("the lane has room for the change");
                let emitted = emit(&mut changed, 0, AT + SETTLE + WINDOW as u64);
                let cue = ENGAGE + (AT - ENGAGE) * 5 / 4;
                assert_eq!(
                    emitted
                        .iter()
                        .find(|chunk| chunk.lane_start == AT)
                        .map(|chunk| chunk.source_start),
                    Some(cue),
                    "the engine change starts on its exact lane and source frame",
                );

                let (mut started, mut fresh) = stretch_lane(&pools, (next, false), &signal);
                fresh
                    .send(
                        When::At(LaneFrame {
                            segment: SegmentId::FIRST,
                            frame: ENGAGE,
                        }),
                        speed_batch(1.25),
                    )
                    .expect("the lane has room for the reference history");
                let mut start = speed_batch(1.25);
                start.commands.push(LaneCommand::SetKeylock(true));
                fresh
                    .send(
                        When::At(LaneFrame {
                            segment: SegmentId::FIRST,
                            frame: AT,
                        }),
                        start,
                    )
                    .expect("the lane has room for the start");
                let reference = emit(&mut started, 0, AT + SETTLE + (WINDOW + REACH) as u64);
                assert_eq!(
                    reference
                        .iter()
                        .find(|chunk| chunk.lane_start == AT)
                        .map(|chunk| chunk.source_start),
                    Some(cue),
                    "the fresh engine starts on the same lane and source frame",
                );

                let rendered = &lane_pcm(&emitted)[pcm_index(AT + SETTLE)..][..WINDOW];
                let (offset, correlation) = alignment(
                    rendered,
                    &lane_pcm(&reference),
                    pcm_index(AT + SETTLE),
                    REACH,
                );
                assert!(
                    offset == 0 && correlation > 0.95,
                    "{command:?}: after the change at lane frame {AT} (source {cue}) the lane \
                     renders {offset} frames off a fresh {to:?} engine, correlation {correlation:.3}"
                );
                assert!(
                    lane.receipts().any(|receipt| receipt.seq() == seq
                        && matches!(
                            receipt.outcome(),
                            Outcome::Applied {
                                at: LaneFrame {
                                    segment: SegmentId::FIRST,
                                    frame: AT
                                },
                                ..
                            }
                        )),
                    "the change's receipt names its frame"
                );
            }
        }
    }

    /// An engine batch and a speed batch on the same lane frame X of a lane at
    /// unity render from X on what the new engine started at X with the new
    /// speed renders: the old engine never renders the new speed.
    #[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
    #[kithara::test]
    fn an_engine_and_a_speed_batch_on_one_frame_engage_the_new_engine() {
        for backend in keylock_backends() {
            const AT: u64 = 4_096;
            const SETTLE: u64 = 16_384;
            const WINDOW: usize = 4_096;
            const REACH: usize = 2_048;
            let signal = chirp(12 * consts::LANE_CHUNK_FRAMES as usize);
            let pools = pools();
            let (mut changed, mut lane) = stretch_lane(&pools, (backend, false), &signal);
            lane.send(
                When::At(LaneFrame {
                    segment: SegmentId::FIRST,
                    frame: AT,
                }),
                command_batch(LaneCommand::SetKeylock(true)),
            )
            .expect("the lane has room for the engine change");
            lane.send(
                When::At(LaneFrame {
                    segment: SegmentId::FIRST,
                    frame: AT,
                }),
                speed_batch(1.25),
            )
            .expect("the lane has room for the speed change");
            let emitted = emit(&mut changed, 0, AT + SETTLE + WINDOW as u64);

            let (mut started, mut fresh) = stretch_lane(&pools, (backend, true), &signal);
            fresh
                .send(
                    When::At(LaneFrame {
                        segment: SegmentId::FIRST,
                        frame: AT,
                    }),
                    speed_batch(1.25),
                )
                .expect("the lane has room for the start");
            let reference = emit(&mut started, 0, AT + SETTLE + (WINDOW + REACH) as u64);

            let rendered = &lane_pcm(&emitted)[pcm_index(AT + SETTLE)..][..WINDOW];
            let (offset, correlation) = alignment(
                rendered,
                &lane_pcm(&reference),
                pcm_index(AT + SETTLE),
                REACH,
            );
            assert!(
                offset == 0 && correlation > 0.95,
                "{backend:?}: after keylock and speed at lane frame {AT} the lane renders \
                 {offset} frames off a fresh keylock engine, correlation {correlation:.3}"
            );
        }
    }
    #[cfg(feature = "stretch-identity")]
    #[kithara::test]
    fn identity_source_preserves_buffers_despite_initial_and_live_controls(quarter: Vec<f32>) {
        let spec = AudioSpec::new(2, NonZeroU32::new(48_000).expect("sample rate"));
        let pools = pools();
        let chunks = [
            chunk_with_frames(&pools, spec, 0, 64, &quarter),
            chunk_with_frames(&pools, spec, 64, 64, &quarter),
        ];
        let pointers = chunks.each_ref().map(|chunk| chunk.samples.as_ptr());
        let raw = RawSource {
            chunks: VecDeque::from(chunks),
            head: Arc::new(AtomicU64::new(0)),
        };
        let config = kithara_warp::WarpConfig::builder()
            .backend(StretchKind::Identity)
            .speed(0.5)
            .keylock(true)
            .render_quantum_frames(NonZeroUsize::new(16).expect("quantum"))
            .build();
        let renderer = kithara_warp::Warp::new((), &config).renderer(spec, pools.clone());
        let drain = EffectDrain::new(0, &pools).expect("empty effect drain");
        let (mut lane, inbox) = channel::<LaneProtocol>(ChannelConfig::builder().build());
        let mut source = WarpSource::new(
            raw,
            renderer,
            Vec::new(),
            drain,
            spec,
            pools.clone(),
            LaneSetup {
                inbox,
                preload_chunks: NonZeroUsize::new(1).expect("preload"),
                declick: consts::DEFAULT_DECLICK,
            },
        );
        for (index, pointer) in pointers.into_iter().enumerate() {
            if index == 1 {
                lane.send(
                    When::Next,
                    Batch {
                        basis: Vec::new(),
                        commands: vec![
                            LaneCommand::SetSpeed(SpeedCurve::Constant(2.0)),
                            LaneCommand::SetKeylock(false),
                        ],
                    },
                )
                .expect("the lane has room");
            }
            let TrackStep::Produced(Fetch::Data {
                data, source_end, ..
            }) = source.step_track()
            else {
                panic!("Identity emits each original chunk immediately");
            };
            assert_eq!(data.samples.as_ptr(), pointer);
            assert_eq!(data.spec(), spec);
            assert_eq!(data.frames(), 64);
            assert_eq!(&*data.samples, &quarter[..128]);
            assert_eq!(
                data.meta.frame_offset,
                u64::try_from(index).expect("index") * 64
            );
            assert_eq!(
                source_end,
                Some(SourceEnd::new(
                    u64::try_from(index + 1).expect("index") * 64,
                    spec.sample_rate,
                ))
            );
            assert!(!source.warp.requires_staging());
            flush_deferred(&mut source);
        }
        let mut ended = false;
        for _ in 0..8 {
            match source.step_track() {
                TrackStep::StateChanged => flush_deferred(&mut source),
                TrackStep::Eof => {
                    ended = true;
                    break;
                }
                _ => panic!("Identity owns no tail and finishes without failure"),
            }
        }
        assert!(ended, "Identity reaches EOF");
    }

    #[cfg(all(
        feature = "stretch-identity",
        any(
            feature = "stretch-signalsmith",
            feature = "stretch-bungee",
            feature = "stretch-glide"
        )
    ))]
    #[kithara::test]
    #[cfg_attr(
        feature = "stretch-signalsmith",
        case::signalsmith(StretchKind::Signalsmith)
    )]
    #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
    #[cfg_attr(feature = "stretch-glide", case::glide(StretchKind::Glide))]
    fn scheduled_identity_native_changes_preserve_the_decoded_suffix(
        #[case] backend: StretchKind,
        quarter: Vec<f32>,
    ) {
        const ENGAGE: u64 = 1_000;
        const RETURN: u64 = 2_000;
        // A scheduled 2x landing rounds 1,999 source frames to 1,000 output frames.
        const RATE_SOURCE_FRAMES: u64 = 1_999;
        let pools = pools();
        let (mut source, mut lane) =
            lane_over(&pools, 3, |_| &quarter, 2.0, (StretchKind::Identity, false));
        let first = lane
            .send(
                When::At(LaneFrame {
                    segment: SegmentId::FIRST,
                    frame: ENGAGE,
                }),
                command_batch(LaneCommand::SetBackend(backend)),
            )
            .expect("the lane has room");
        let second = lane
            .send(
                When::At(LaneFrame {
                    segment: SegmentId::FIRST,
                    frame: RETURN,
                }),
                command_batch(LaneCommand::SetBackend(StretchKind::Identity)),
            )
            .expect("the lane has room");
        let expected_output =
            u64::from(3 * consts::LANE_CHUNK_FRAMES) - RATE_SOURCE_FRAMES + (RETURN - ENGAGE);
        let emitted = emit(&mut source, 0, expected_output);
        let actual_frames = emitted
            .iter()
            .map(|chunk| chunk.samples.len() / 2)
            .sum::<usize>();
        assert_eq!(
            actual_frames,
            usize::try_from(expected_output).expect("output frames")
        );
        for chunk in &emitted {
            let expected_source = if chunk.lane_start < ENGAGE {
                chunk.lane_start
            } else if chunk.lane_start < RETURN {
                ENGAGE + (chunk.lane_start - ENGAGE) * 2
            } else {
                chunk.lane_start + RATE_SOURCE_FRAMES - (RETURN - ENGAGE)
            };
            if (ENGAGE..RETURN).contains(&chunk.lane_start) {
                assert!(
                    chunk.source_start.abs_diff(expected_source) <= 1,
                    "backend changes preserve source position at lane frame {}: {} vs {expected_source}",
                    chunk.lane_start,
                    chunk.source_start
                );
            } else {
                assert_eq!(chunk.source_start, expected_source);
            }
            if chunk.lane_start >= RETURN {
                assert!(
                    chunk.samples.iter().all(|sample| *sample == quarter[0]),
                    "the held Identity suffix remains unchanged"
                );
            }
        }
        assert_eq!(
            source.source.head.load(Ordering::Acquire),
            u64::from(3 * consts::LANE_CHUNK_FRAMES)
        );
        assert!(
            source.pending_input.is_none(),
            "every decoded suffix was consumed"
        );
        assert_eq!(
            source.warp.rendered_source_end().map(|(frame, _)| frame),
            Some(u64::from(3 * consts::LANE_CHUNK_FRAMES))
        );
        let mut ended = false;
        for _ in 0..8 {
            match source.step_track() {
                TrackStep::StateChanged => flush_deferred(&mut source),
                TrackStep::Eof => {
                    ended = true;
                    break;
                }
                _ => panic!("the complete Identity suffix leaves no output tail"),
            }
        }
        assert!(ended, "the complete source reaches EOF");
        let outcomes = lane
            .receipts()
            .map(|receipt| {
                let seq = receipt.seq();
                let (outcome, _) = receipt.into();
                (seq, outcome)
            })
            .collect::<Vec<_>>();
        assert!(matches!(outcomes.as_slice(), [
            (engage, Outcome::Applied { at: LaneFrame { segment: SegmentId::FIRST, frame: ENGAGE }, .. }),
            (identity, Outcome::Applied { at: LaneFrame { segment: SegmentId::FIRST, frame: RETURN }, .. }),
        ] if *engage == first && *identity == second));
    }
}
