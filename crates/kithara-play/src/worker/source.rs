use kithara_audio::{AudioSource, Fetch, SourceDiscontinuity, SourceEnd, TrackStep};
use kithara_bufpool::{SampleBuffer, SamplePool};
use kithara_platform::sync::Arc;
use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec};
use kithara_stream::SeekObserve;

use crate::effects::{
    AudioEffect, EffectDrain, EffectDrainStep, apply_effects, held_source_frames, reset_effects,
};

#[derive(Clone, Copy)]
enum DrainState {
    Open,
    Warp(u64),
    Effects(u64),
    Exhausted(u64),
}

impl DrainState {
    const fn epoch(self) -> Option<u64> {
        match self {
            Self::Open => None,
            Self::Warp(epoch) | Self::Effects(epoch) | Self::Exhausted(epoch) => Some(epoch),
        }
    }
}

struct PendingInput {
    chunk: AudioChunk,
    consumed_frames: usize,
    epoch: u64,
}

#[derive(Clone, Copy)]
struct PreparedQuantum {
    meta: AudioChunkInfo,
    frames: usize,
    samples: usize,
    whole_input: bool,
}

/// The sole producer-side Warp/effect stage before the play output ring.
pub(crate) struct WarpSource<S> {
    source: S,
    warp: kithara_warp::WarpRenderer,
    effects: Vec<Box<dyn AudioEffect>>,
    drain: EffectDrain,
    seek: Arc<dyn SeekObserve>,
    discontinuity: Option<SourceDiscontinuity>,
    spec: AudioSpec,
    drain_state: DrainState,
    reset_epoch: Option<u64>,
    sample_pool: SamplePool,
    pending_input: Option<PendingInput>,
    quantum_input: Option<SampleBuffer>,
    prepared_quantum: Option<PreparedQuantum>,
    retired_input: Option<AudioChunk>,
    quantum_failed: bool,
}

impl<S> WarpSource<S>
where
    S: AudioSource<Chunk = AudioChunk>,
{
    pub(crate) fn new(
        source: S,
        warp: kithara_warp::WarpRenderer,
        effects: Vec<Box<dyn AudioEffect>>,
        drain: EffectDrain,
        spec: AudioSpec,
        sample_pool: SamplePool,
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
            drain_state: DrainState::Open,
            reset_epoch: None,
            sample_pool,
            pending_input: None,
            quantum_input: None,
            prepared_quantum: None,
            retired_input: None,
            quantum_failed: false,
        }
    }

    fn retire_pending_input(&mut self) {
        let Some(pending) = self.pending_input.take() else {
            return;
        };
        debug_assert!(self.retired_input.is_none());
        self.retired_input = Some(pending.chunk);
        self.prepared_quantum = None;
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
        self.retire_pending_input();
        self.quantum_failed = false;
        if already_reset {
            self.reset_epoch = None;
        } else {
            self.reset_renderers();
        }
        self.drain.reset();
        self.drain_state = DrainState::Open;
    }

    fn reset_renderers(&mut self) {
        self.warp.reset();
        reset_effects(&mut self.effects);
    }

    fn prepare_renderers(&mut self, spec: AudioSpec) {
        self.spec = spec;
        self.warp.prepare(spec);
        for effect in &mut self.effects {
            effect.service_deferred(spec);
        }
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
        });
        if !stale {
            return false;
        }
        let epoch = self.seek.epoch();
        self.retire_pending_input();
        self.reset_renderers();
        self.drain.reset();
        self.drain_state = DrainState::Open;
        self.reset_epoch = Some(epoch);
        self.quantum_failed = false;
        true
    }

    fn pending_span_meta(pending: &PendingInput, frames: usize) -> Option<AudioChunkInfo> {
        let original = pending.chunk.meta;
        let consumed = u64::try_from(pending.consumed_frames).ok()?;
        let frames = u32::try_from(frames).ok()?;
        let mut meta = original;
        meta.frame_offset = original.frame_offset.checked_add(consumed)?;
        meta.timestamp = original
            .timestamp
            .checked_add(original.spec.duration_for(consumed).ok()?)?;
        meta.frames = frames;
        meta.end_timestamp = meta
            .timestamp
            .checked_add(original.spec.duration_for(u64::from(frames)).ok()?)?;
        let total_frames = pending.chunk.frames();
        let span_end = pending
            .consumed_frames
            .checked_add(usize::try_from(frames).ok()?)?;
        if span_end == total_frames {
            meta.end_timestamp = original.end_timestamp;
        }
        if pending.consumed_frames > 0 || usize::try_from(frames).ok()? != total_frames {
            meta.source_byte_offset = None;
            meta.source_bytes = 0;
        }
        Some(meta)
    }

    fn prepare_quantum_shape(&mut self) -> Option<PreparedQuantum> {
        let pending = self.pending_input.as_ref()?;
        let total_frames = pending.chunk.frames();
        let remaining = total_frames.checked_sub(pending.consumed_frames)?;
        let current = Self::pending_span_meta(pending, remaining)?;
        let frames = self.warp.prepare_quantum(current, remaining)?.get();
        let samples = frames.checked_mul(usize::from(current.spec.channels.max(1)))?;
        Some(PreparedQuantum {
            meta: Self::pending_span_meta(pending, frames)?,
            frames,
            samples,
            whole_input: pending.consumed_frames == 0 && frames == total_frames,
        })
    }

    fn prepare_quantum_input(&mut self) {
        if self.quantum_failed || self.pending_input.is_none() {
            self.prepared_quantum = None;
            return;
        }
        let Some(prepared) = self.prepare_quantum_shape() else {
            self.quantum_failed = true;
            return;
        };
        self.prepared_quantum = Some(prepared);
        if prepared.whole_input {
            return;
        }
        let mut input = self
            .quantum_input
            .take()
            .unwrap_or_else(|| self.sample_pool.get());
        if input.ensure_len(prepared.samples).is_err() {
            self.quantum_input = Some(input);
            self.quantum_failed = true;
            return;
        }
        self.quantum_input = Some(input);
    }

    fn render_pending(&mut self) -> TrackStep<AudioChunk> {
        if self.quantum_failed {
            return TrackStep::Failed;
        }
        let Some(prepared) = self.prepared_quantum.take() else {
            return TrackStep::StateChanged;
        };
        if prepared.whole_input {
            let Some(pending) = self.pending_input.take() else {
                self.quantum_failed = true;
                return TrackStep::Failed;
            };
            debug_assert_eq!(pending.consumed_frames, 0);
            debug_assert_eq!(pending.chunk.frames(), prepared.frames);
            return self
                .render(pending.chunk, pending.epoch)
                .map_or(TrackStep::StateChanged, TrackStep::Produced);
        }
        let Some(mut input) = self.quantum_input.take() else {
            return TrackStep::StateChanged;
        };
        if input.len() < prepared.samples {
            self.quantum_input = Some(input);
            return TrackStep::StateChanged;
        }

        let Some(pending) = self.pending_input.as_mut() else {
            self.quantum_input = Some(input);
            return TrackStep::Failed;
        };
        let channels = usize::from(prepared.meta.spec.channels.max(1));
        let start = pending.consumed_frames.checked_mul(channels);
        let end = start.and_then(|start| start.checked_add(prepared.samples));
        let Some((start, end)) = start.zip(end) else {
            self.quantum_input = Some(input);
            self.quantum_failed = true;
            return TrackStep::Failed;
        };
        let Some(source) = pending.chunk.samples.get(start..end) else {
            self.quantum_input = Some(input);
            self.quantum_failed = true;
            return TrackStep::Failed;
        };
        input.truncate(prepared.samples);
        input.copy_from_slice(source);
        let Some(consumed_frames) = pending.consumed_frames.checked_add(prepared.frames) else {
            self.quantum_input = Some(input);
            self.quantum_failed = true;
            return TrackStep::Failed;
        };
        pending.consumed_frames = consumed_frames;
        let epoch = pending.epoch;
        if pending.consumed_frames == pending.chunk.frames() {
            self.retire_pending_input();
        }

        self.render(AudioChunk::new(prepared.meta, input), epoch)
            .map_or(TrackStep::StateChanged, TrackStep::Produced)
    }

    fn begin_drain(&mut self, epoch: u64) {
        self.drain_state = DrainState::Warp(epoch);
    }

    fn fetch(&self, data: AudioChunk, epoch: u64) -> Fetch<AudioChunk> {
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

    fn render(&mut self, chunk: AudioChunk, epoch: u64) -> Option<Fetch<AudioChunk>> {
        let chunk = self.warp.render_quantum(chunk);
        let chunk = chunk?;
        let output = apply_effects(&mut self.effects, chunk)?;
        Some(self.fetch(output, epoch))
    }

    fn drain_step(&mut self) -> Option<TrackStep<AudioChunk>> {
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
}

impl<S> AudioSource for WarpSource<S>
where
    S: AudioSource<Chunk = AudioChunk>,
{
    type Chunk = AudioChunk;

    delegate::delegate! {
        to self.source {
            fn decode_epoch(&self) -> u64;
            fn commit_source_end(&mut self, source_end: SourceEnd, epoch: u64);
            fn retire_chunk(&self, chunk: AudioChunk);
            fn finish_deferred(&mut self);
            fn warm_up(&mut self);
        }
    }

    fn discontinuity(&self) -> Option<SourceDiscontinuity> {
        self.discontinuity
    }

    fn seek_observe(&self) -> Arc<dyn SeekObserve> {
        Arc::clone(&self.seek)
    }

    #[cfg_attr(feature = "perf", hotpath::measure)]
    fn step_track(&mut self) -> TrackStep<AudioChunk> {
        self.sync_discontinuity();
        if self.cancel_stale_input() {
            return TrackStep::StateChanged;
        }
        if self.cancel_stale_drain() {
            return TrackStep::StateChanged;
        }

        if matches!(self.drain_state, DrainState::Exhausted(_)) {
            return TrackStep::Eof;
        }
        if let Some(step) = self.drain_step() {
            return step;
        }
        if self.pending_input.is_some() {
            return self.render_pending();
        }
        if !self.warp.accepts_input() {
            return TrackStep::Failed;
        }

        match self.source.step_track() {
            TrackStep::Produced(Fetch::Data { data, epoch, .. }) => {
                let same_spec = data.spec() == self.spec;
                self.pending_input = Some(PendingInput {
                    chunk: data,
                    consumed_frames: 0,
                    epoch,
                });
                if same_spec {
                    self.prepared_quantum = self.prepare_quantum_shape();
                    if self
                        .prepared_quantum
                        .is_some_and(|prepared| prepared.whole_input)
                    {
                        return self.render_pending();
                    }
                }
                TrackStep::StateChanged
            }
            TrackStep::Produced(fetch) => TrackStep::Produced(fetch),
            TrackStep::Eof => {
                self.begin_drain(self.source.decode_epoch());
                TrackStep::StateChanged
            }
            TrackStep::StateChanged => {
                self.sync_discontinuity();
                TrackStep::StateChanged
            }
            TrackStep::Blocked(reason) => TrackStep::Blocked(reason),
            TrackStep::Failed => TrackStep::Failed,
        }
    }

    #[cfg_attr(feature = "perf", hotpath::measure)]
    fn prepare_deferred(&mut self) -> Option<AudioSpec> {
        if let Some(chunk) = self.retired_input.take() {
            self.source.retire_chunk(chunk);
        }
        let spec = self.source.prepare_deferred();
        self.sync_discontinuity();
        self.prepare_renderers(spec.unwrap_or(self.spec));
        self.prepare_quantum_input();
        spec
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, num::NonZeroU32};

    use kithara_audio::{Fetch, TrackStep, WaitingReason};
    use kithara_bufpool::{ByteBudget, BytePool, SamplePool};
    use kithara_platform::sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    };
    use kithara_signal::AudioChunkInfo;
    use kithara_stream::{SeekControl, SeekObserve, SeekState};
    use kithara_test_utils::kithara;
    use kithara_warp::{StretchControls, StretchKind};

    use super::*;

    fn flush_deferred<S>(source: &mut S)
    where
        S: AudioSource,
    {
        let _ = source.prepare_deferred();
        source.finish_deferred();
    }

    fn source_stage<S>(
        source: S,
        effects: Vec<Box<dyn AudioEffect>>,
        drain: EffectDrain,
        spec: AudioSpec,
    ) -> WarpSource<S>
    where
        S: AudioSource<Chunk = AudioChunk>,
    {
        let config = kithara_warp::WarpConfig::builder().build();
        let warp = kithara_warp::Warp::new((), &config);
        let sample_pool = SamplePool::default();
        let renderer = warp.renderer(spec, sample_pool.clone());
        WarpSource::new(source, renderer, effects, drain, spec, sample_pool)
    }

    struct RawSource {
        chunks: VecDeque<AudioChunk>,
        head: Arc<AtomicU64>,
        seek: Arc<SeekState>,
    }

    impl AudioSource for RawSource {
        type Chunk = AudioChunk;

        fn seek_observe(&self) -> Arc<dyn SeekObserve> {
            Arc::clone(&self.seek) as Arc<dyn SeekObserve>
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
            TrackStep::Produced(Fetch::data(chunk, self.seek.epoch()))
        }
    }

    #[derive(Default)]
    struct BufferThenHalveFrames {
        buffered: Option<AudioChunk>,
    }

    impl AudioEffect for BufferThenHalveFrames {
        fn flush(&mut self) -> Option<AudioChunk> {
            self.buffered.take().and_then(halve_frames)
        }

        fn held_source_frames(&self) -> u64 {
            self.buffered
                .as_ref()
                .map_or(0, |chunk| u64::from(chunk.meta.frames))
        }

        fn process(&mut self, chunk: AudioChunk) -> Option<AudioChunk> {
            self.buffered.replace(chunk).and_then(halve_frames)
        }

        fn reset(&mut self) {
            self.buffered = None;
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
        seek: Arc<SeekState>,
        spec: AudioSpec,
    }

    impl AudioSource for DeferredSource {
        type Chunk = AudioChunk;

        fn prepare_deferred(&mut self) -> Option<AudioSpec> {
            self.log.lock().push("source.prepare");
            Some(self.spec)
        }

        fn finish_deferred(&mut self) {
            self.log.lock().push("source.finish");
        }

        fn seek_observe(&self) -> Arc<dyn SeekObserve> {
            Arc::clone(&self.seek) as Arc<dyn SeekObserve>
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
        fn service_deferred(&mut self, spec: AudioSpec) {
            self.log.lock().push("effect.service");
            *self.serviced.lock() = Some(spec);
        }

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
    }

    struct RevisionSource {
        chunks: VecDeque<AudioChunk>,
        discontinuity: Arc<Mutex<SourceDiscontinuity>>,
        seek: Arc<SeekState>,
    }

    impl AudioSource for RevisionSource {
        type Chunk = AudioChunk;

        fn discontinuity(&self) -> Option<SourceDiscontinuity> {
            Some(*self.discontinuity.lock())
        }

        fn seek_observe(&self) -> Arc<dyn SeekObserve> {
            Arc::clone(&self.seek) as Arc<dyn SeekObserve>
        }

        fn step_track(&mut self) -> TrackStep<AudioChunk> {
            self.chunks
                .pop_front()
                .map_or(TrackStep::Blocked(WaitingReason::Waiting), |chunk| {
                    TrackStep::Produced(Fetch::data(chunk, 0))
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
        seek: Arc<SeekState>,
    }

    impl AudioSource for CountingEofSource {
        type Chunk = AudioChunk;

        fn discontinuity(&self) -> Option<SourceDiscontinuity> {
            *self.discontinuity.lock()
        }

        fn seek_observe(&self) -> Arc<dyn SeekObserve> {
            Arc::clone(&self.seek) as Arc<dyn SeekObserve>
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
        decode_epoch: u64,
        revision: u64,
        seek: Arc<SeekState>,
        spec: AudioSpec,
    }

    impl AudioSource for SeekApplyingSource {
        type Chunk = AudioChunk;

        fn decode_epoch(&self) -> u64 {
            self.decode_epoch
        }

        fn discontinuity(&self) -> Option<SourceDiscontinuity> {
            Some(SourceDiscontinuity::new(self.revision, self.spec))
        }

        fn seek_observe(&self) -> Arc<dyn SeekObserve> {
            Arc::clone(&self.seek) as Arc<dyn SeekObserve>
        }

        fn step_track(&mut self) -> TrackStep<AudioChunk> {
            let live_epoch = self.seek.epoch();
            if live_epoch != self.decode_epoch {
                self.decode_epoch = live_epoch;
                self.revision = self.revision.wrapping_add(1);
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

    fn chunk(pool: &SamplePool, spec: AudioSpec, frame_offset: u64) -> AudioChunk {
        const FRAMES: usize = 128;
        chunk_with_frames(
            pool,
            spec,
            frame_offset,
            u32::try_from(FRAMES).expect("fixture frames fit u32"),
            0.25,
        )
    }

    fn chunk_with_frames(
        pool: &SamplePool,
        spec: AudioSpec,
        frame_offset: u64,
        frames: u32,
        sample: f32,
    ) -> AudioChunk {
        let samples = usize::try_from(frames)
            .expect("fixture frames fit usize")
            .checked_mul(usize::from(spec.channels))
            .expect("fixture sample count fits usize");
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
            pool.attach(vec![sample; samples]),
        )
    }

    #[kithara::test]
    fn buffered_frame_changing_effect_tracks_live_and_flush_frontiers() {
        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("test sample rate"));
        let pool = SamplePool::default();
        let head = Arc::new(AtomicU64::new(0));
        let source = RawSource {
            chunks: VecDeque::from([
                chunk(&pool, spec, 0),
                chunk(&pool, spec, u64::from(128_u32)),
            ]),
            head: Arc::clone(&head),
            seek: Arc::new(SeekState::new()),
        };
        let effects: Vec<Box<dyn AudioEffect>> = vec![Box::<BufferThenHalveFrames>::default()];
        let drain = EffectDrain::new(effects.len(), &BytePool::default());
        let mut source = source_stage(source, effects, drain, spec);

        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert_eq!(
            head.load(Ordering::Acquire),
            128,
            "one worker pass advances exactly one source transition"
        );
        flush_deferred(&mut source);
        let TrackStep::Produced(Fetch::Data {
            data, source_end, ..
        }) = source.step_track()
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
        let log = Arc::new(Mutex::new(Vec::new()));
        let serviced = Arc::new(Mutex::new(None));
        let source = DeferredSource {
            log: Arc::clone(&log),
            seek: Arc::new(SeekState::new()),
            spec,
        };
        let effects: Vec<Box<dyn AudioEffect>> = vec![Box::new(DeferredEffect {
            log: Arc::clone(&log),
            serviced: Arc::clone(&serviced),
        })];
        let drain = EffectDrain::new(effects.len(), &BytePool::default());
        let mut source = source_stage(source, effects, drain, spec);

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
        let discontinuity = Arc::new(Mutex::new(SourceDiscontinuity::new(7, initial)));
        let resets = Arc::new(AtomicU64::new(0));
        let source = RevisionSource {
            chunks: VecDeque::new(),
            discontinuity: Arc::clone(&discontinuity),
            seek: Arc::new(SeekState::new()),
        };
        let effects: Vec<Box<dyn AudioEffect>> = vec![Box::new(ResetCounter {
            resets: Arc::clone(&resets),
        })];
        let drain = EffectDrain::new(effects.len(), &BytePool::default());
        let mut source = source_stage(source, effects, drain, initial);

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
    fn unity_warp_preserves_samples_and_meta_across_discontinuity() {
        let initial = AudioSpec::new(2, NonZeroU32::new(44_100).expect("initial rate"));
        let changed = AudioSpec::new(1, NonZeroU32::new(48_000).expect("changed rate"));
        let pool = SamplePool::default();
        let first = chunk(&pool, initial, 256);
        let first_ptr = first.samples.as_ptr();
        let first_meta = first.meta;
        let first_samples = first.samples.to_vec();
        let mut second = chunk(&pool, changed, 512);
        second.meta.segment_index = Some(3);
        second.meta.variant_index = Some(2);
        second.meta.epoch = 9;
        second.meta.source_byte_offset = Some(4096);
        second.meta.source_bytes = 1024;
        second.samples.fill(-0.25);
        let second_ptr = second.samples.as_ptr();
        let second_meta = second.meta;
        let second_samples = second.samples.to_vec();
        let discontinuity = Arc::new(Mutex::new(SourceDiscontinuity::new(7, initial)));
        let source = RevisionSource {
            chunks: VecDeque::from([first, second]),
            discontinuity: Arc::clone(&discontinuity),
            seek: Arc::new(SeekState::new()),
        };
        let effects = Vec::new();
        let drain = EffectDrain::new(effects.len(), &BytePool::default());
        let mut source = source_stage(source, effects, drain, initial);

        let TrackStep::Produced(Fetch::Data { data, .. }) = source.step_track() else {
            panic!("initial unity span must pass through");
        };
        assert_eq!(data.meta, first_meta);
        assert_eq!(data.samples.as_ptr(), first_ptr);
        assert_eq!(&data.samples[..], &first_samples);

        *discontinuity.lock() = SourceDiscontinuity::new(8, changed);
        flush_deferred(&mut source);
        let TrackStep::Produced(Fetch::Data { data, .. }) = source.step_track() else {
            panic!("post-discontinuity unity span must pass through");
        };
        assert_eq!(data.meta, second_meta);
        assert_eq!(data.samples.as_ptr(), second_ptr);
        assert_eq!(&data.samples[..], &second_samples);
    }

    #[kithara::test]
    #[cfg_attr(
        feature = "stretch-signalsmith",
        case::signalsmith(StretchKind::Signalsmith)
    )]
    #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
    fn live_rate_changes_stay_bounded_and_seek_discards_stale_input(#[case] backend: StretchKind) {
        const ACTIVE_FRAMES: u32 = 4096;
        const RATE_CHANGE_FRAMES: u32 = 4096;
        const SENTINEL_FRAMES: u32 = 512;

        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("test sample rate"));
        let pool = SamplePool::default();
        let first_active_end = u64::from(ACTIVE_FRAMES);
        let rate_change_end = first_active_end.saturating_add(u64::from(RATE_CHANGE_FRAMES));
        let sentinel_end = rate_change_end.saturating_add(u64::from(SENTINEL_FRAMES));

        let first_active = chunk_with_frames(&pool, spec, 0, ACTIVE_FRAMES, 0.25);
        let rate_change =
            chunk_with_frames(&pool, spec, first_active_end, RATE_CHANGE_FRAMES, -0.25);
        let sentinel = chunk_with_frames(&pool, spec, rate_change_end, SENTINEL_FRAMES, 0.75);
        let sentinel_ptr = sentinel.samples.as_ptr();
        let sentinel_samples = sentinel.samples.to_vec();

        let head = Arc::new(AtomicU64::new(0));
        let seek = Arc::new(SeekState::new());
        let raw = RawSource {
            chunks: VecDeque::from([first_active, rate_change, sentinel]),
            head: Arc::clone(&head),
            seek: Arc::clone(&seek),
        };
        let controls = StretchControls::new(0.5);
        controls.set_keylock(true);
        controls.set_backend(backend);
        let config = kithara_warp::WarpConfig::builder()
            .stretch(Arc::clone(&controls))
            .build();
        let renderer = kithara_warp::Warp::new((), &config).renderer(spec, pool.clone());
        let effects = Vec::new();
        let drain = EffectDrain::new(effects.len(), &BytePool::default());
        let mut source = WarpSource::new(raw, renderer, effects, drain, spec, pool);

        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert_eq!(head.load(Ordering::Acquire), first_active_end);
        let mut active_steps = 0;
        while source.pending_input.is_some() {
            flush_deferred(&mut source);
            let step = source.step_track();
            if let TrackStep::Produced(Fetch::Data { data, .. }) = &step {
                assert!(
                    data.frames() <= 512,
                    "live Warp output stays quantum-bounded"
                );
            }
            assert!(matches!(
                step,
                TrackStep::Produced(_) | TrackStep::StateChanged
            ));
            active_steps += 1;
            assert!(active_steps < 64, "bounded active input must converge");
        }
        assert!(active_steps > 1, "fixture must span multiple Warp quanta");

        controls.set_speed(1.0);
        flush_deferred(&mut source);
        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert_eq!(head.load(Ordering::Acquire), rate_change_end);
        flush_deferred(&mut source);
        let TrackStep::Produced(Fetch::Data { data, .. }) = source.step_track() else {
            panic!("the active engine must process exact-unity without a terminal drain");
        };
        assert!(data.frames() <= 512);

        controls.set_speed(2.0);
        let held_head = head.load(Ordering::Acquire);
        flush_deferred(&mut source);
        let TrackStep::Produced(Fetch::Data { data, .. }) = source.step_track() else {
            panic!("the next quantum must observe the latest live rate");
        };
        assert!(data.frames() <= 512);
        assert_eq!(head.load(Ordering::Acquire), held_head);

        controls.set_speed(1.0);
        assert_eq!(
            seek.begin(kithara_platform::time::Duration::from_secs(1)),
            1
        );
        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert_eq!(head.load(Ordering::Acquire), rate_change_end);

        flush_deferred(&mut source);
        let TrackStep::Produced(Fetch::Data { data, .. }) = source.step_track() else {
            panic!("playback must resume with the post-seek source chunk");
        };
        assert_eq!(head.load(Ordering::Acquire), sentinel_end);
        assert_eq!(data.samples.as_ptr(), sentinel_ptr);
        assert_eq!(&data.samples[..], &sentinel_samples);
    }

    #[kithara::test]
    #[cfg_attr(
        feature = "stretch-signalsmith",
        case::signalsmith(StretchKind::Signalsmith)
    )]
    #[cfg_attr(feature = "stretch-bungee", case::bungee(StretchKind::Bungee))]
    fn unavailable_warp_target_fails_before_pulling_source(#[case] backend: StretchKind) {
        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("test sample rate"));
        let head = Arc::new(AtomicU64::new(0));
        let raw = RawSource {
            chunks: VecDeque::from([chunk(&SamplePool::default(), spec, 0)]),
            head: Arc::clone(&head),
            seek: Arc::new(SeekState::new()),
        };
        let controls = StretchControls::new(0.5);
        controls.set_keylock(true);
        controls.set_backend(backend);
        let config = kithara_warp::WarpConfig::builder()
            .stretch(controls)
            .build();
        let target_pool = SamplePool::with_byte_budget(8, 0, ByteBudget(0));
        let renderer = kithara_warp::Warp::new((), &config).renderer(spec, target_pool.clone());
        let failed_prepares = target_pool.stats().budget_overshoots;
        let effects = Vec::new();
        let drain = EffectDrain::new(effects.len(), &BytePool::default());
        let mut source = WarpSource::new(raw, renderer, effects, drain, spec, target_pool.clone());

        for _ in 0..3 {
            flush_deferred(&mut source);
            assert!(matches!(source.step_track(), TrackStep::Failed));
            assert_eq!(head.load(Ordering::Acquire), 0);
        }
        assert_eq!(
            target_pool.stats().budget_overshoots,
            failed_prepares,
            "a persistent target failure must not rebuild on every shell pass"
        );
    }

    #[kithara::test]
    fn seek_cancels_stale_tail_and_resets_effects_once() {
        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("test sample rate"));
        let pool = SamplePool::default();
        let seek = Arc::new(SeekState::new());
        let resets = Arc::new(AtomicU64::new(0));
        let source = SeekApplyingSource {
            decode_epoch: 0,
            revision: 0,
            seek: Arc::clone(&seek),
            spec,
        };
        let effects: Vec<Box<dyn AudioEffect>> = vec![Box::new(ResettingTail {
            resets: Arc::clone(&resets),
            tail: Some(chunk(&pool, spec, 128)),
        })];
        let drain = EffectDrain::new(effects.len(), &BytePool::default());
        let mut source = source_stage(source, effects, drain, spec);

        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert_eq!(
            seek.begin(kithara_platform::time::Duration::from_secs(1)),
            1
        );
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
    fn every_effect_tail_precedes_the_single_eof() {
        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("test sample rate"));
        let pool = SamplePool::default();
        let source = RawSource {
            chunks: VecDeque::new(),
            head: Arc::new(AtomicU64::new(0)),
            seek: Arc::new(SeekState::new()),
        };
        let effects: Vec<Box<dyn AudioEffect>> = vec![
            Box::new(ResettingTail {
                resets: Arc::new(AtomicU64::new(0)),
                tail: Some(chunk(&pool, spec, 128)),
            }),
            Box::new(ResettingTail {
                resets: Arc::new(AtomicU64::new(0)),
                tail: Some(chunk(&pool, spec, 256)),
            }),
        ];
        let drain = EffectDrain::new(effects.len(), &BytePool::default());
        let mut source = source_stage(source, effects, drain, spec);

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
        let steps = Arc::new(AtomicU64::new(0));
        let flushes = Arc::new(AtomicU64::new(0));
        let resets = Arc::new(AtomicU64::new(0));
        let discontinuity = Arc::new(Mutex::new(None));
        let seek = Arc::new(SeekState::new());
        let source = CountingEofSource {
            discontinuity: Arc::clone(&discontinuity),
            steps: Arc::clone(&steps),
            seek: Arc::clone(&seek),
        };
        let effects: Vec<Box<dyn AudioEffect>> = vec![Box::new(CountingEmptyTail {
            flushes: Arc::clone(&flushes),
            resets: Arc::clone(&resets),
        })];
        let drain = EffectDrain::new(effects.len(), &BytePool::default());
        let mut source = source_stage(source, effects, drain, spec);

        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert!(matches!(source.step_track(), TrackStep::Eof));
        for _ in 0..3 {
            assert!(matches!(source.step_track(), TrackStep::Eof));
        }

        assert_eq!(steps.load(Ordering::Acquire), 1);
        assert_eq!(flushes.load(Ordering::Acquire), 1);

        assert_eq!(
            seek.begin(kithara_platform::time::Duration::from_secs(1)),
            1
        );
        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert!(matches!(source.step_track(), TrackStep::Eof));
        assert!(matches!(source.step_track(), TrackStep::Eof));
        assert_eq!(steps.load(Ordering::Acquire), 2);
        assert_eq!(flushes.load(Ordering::Acquire), 2);

        *discontinuity.lock() = Some(SourceDiscontinuity::new(1, spec));
        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert!(matches!(source.step_track(), TrackStep::Eof));
        assert!(matches!(source.step_track(), TrackStep::Eof));
        assert_eq!(steps.load(Ordering::Acquire), 3);
        assert_eq!(flushes.load(Ordering::Acquire), 3);
        assert_eq!(resets.load(Ordering::Acquire), 1);
    }
}
