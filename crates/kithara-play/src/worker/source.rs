use kithara_audio::{Fetch, PcmSource, SourceDiscontinuity, TrackStep};
use kithara_decode::{PcmChunk, PcmSpec};
use kithara_platform::sync::Arc;
use kithara_stream::SeekObserve;

use crate::effects::{AudioEffect, EffectDrain, EffectDrainStep, apply_effects, reset_effects};

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

/// The sole producer-side Warp/effect stage before the play output ring.
pub(crate) struct WarpSource<S> {
    source: S,
    #[cfg(not(target_arch = "wasm32"))]
    warp: kithara_warp::WarpRenderer,
    effects: Vec<Box<dyn AudioEffect>>,
    drain: EffectDrain,
    seek: Arc<dyn SeekObserve>,
    discontinuity: Option<SourceDiscontinuity>,
    spec: PcmSpec,
    drain_state: DrainState,
    reset_epoch: Option<u64>,
}

impl<S> WarpSource<S>
where
    S: PcmSource<Chunk = PcmChunk>,
{
    pub(crate) fn new(
        source: S,
        #[cfg(not(target_arch = "wasm32"))] warp: kithara_warp::WarpRenderer,
        effects: Vec<Box<dyn AudioEffect>>,
        drain: EffectDrain,
        spec: PcmSpec,
    ) -> Self {
        let discontinuity = source.discontinuity();
        let seek = source.seek_observe();
        Self {
            source,
            #[cfg(not(target_arch = "wasm32"))]
            warp,
            effects,
            drain,
            seek,
            discontinuity,
            spec,
            drain_state: DrainState::Open,
            reset_epoch: None,
        }
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
        if already_reset {
            self.reset_epoch = None;
        } else {
            self.reset_renderers();
        }
        self.drain.reset();
        self.drain_state = DrainState::Open;
    }

    fn reset_renderers(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.warp.reset();
        reset_effects(&mut self.effects);
    }

    fn prepare_renderers(&mut self, spec: PcmSpec) {
        self.spec = spec;
        #[cfg(not(target_arch = "wasm32"))]
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

    fn begin_drain(&mut self, epoch: u64) {
        self.drain_state = DrainState::Warp(epoch);
    }

    fn render(&mut self, chunk: PcmChunk) -> Option<PcmChunk> {
        #[cfg(not(target_arch = "wasm32"))]
        let chunk = self.warp.render(chunk)?;
        apply_effects(&mut self.effects, chunk)
    }

    fn drain_step(&mut self) -> Option<TrackStep<PcmChunk>> {
        if let DrainState::Warp(epoch) = self.drain_state {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(chunk) = self.warp.flush() {
                return Some(
                    apply_effects(&mut self.effects, chunk)
                        .map_or(TrackStep::StateChanged, |output| {
                            TrackStep::Produced(Fetch::data(output, epoch))
                        }),
                );
            }
            self.drain_state = DrainState::Effects(epoch);
        }

        let DrainState::Effects(epoch) = self.drain_state else {
            return None;
        };
        Some(match self.drain.step(&mut self.effects) {
            EffectDrainStep::Produced(chunk) => TrackStep::Produced(Fetch::data(chunk, epoch)),
            EffectDrainStep::Progress => TrackStep::StateChanged,
            EffectDrainStep::Exhausted => {
                self.drain_state = DrainState::Exhausted(epoch);
                TrackStep::Eof
            }
        })
    }
}

impl<S> PcmSource for WarpSource<S>
where
    S: PcmSource<Chunk = PcmChunk>,
{
    type Chunk = PcmChunk;

    delegate::delegate! {
        to self.source {
            fn decode_epoch(&self) -> u64;
            fn retire_chunk(&self, chunk: PcmChunk);
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

    fn step_track(&mut self) -> TrackStep<PcmChunk> {
        self.sync_discontinuity();
        if self.cancel_stale_drain() {
            return TrackStep::StateChanged;
        }

        if matches!(self.drain_state, DrainState::Exhausted(_)) {
            return TrackStep::Eof;
        }
        if let Some(step) = self.drain_step() {
            return step;
        }

        match self.source.step_track() {
            TrackStep::Produced(Fetch::Data { data, epoch }) => {
                self.render(data).map_or(TrackStep::StateChanged, |output| {
                    TrackStep::Produced(Fetch::data(output, epoch))
                })
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

    fn prepare_deferred(&mut self) -> Option<PcmSpec> {
        let spec = self.source.prepare_deferred();
        self.sync_discontinuity();
        self.prepare_renderers(spec.unwrap_or(self.spec));
        spec
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, num::NonZeroU32};

    use kithara_audio::{Fetch, TrackStep, WaitingReason};
    use kithara_bufpool::{BytePool, PcmPool};
    use kithara_decode::{PcmMeta, duration_for_frames};
    use kithara_platform::sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    };
    use kithara_stream::{SeekControl, SeekObserve, SeekState};
    use kithara_test_utils::kithara;

    use super::*;

    fn flush_deferred<S>(source: &mut S)
    where
        S: PcmSource,
    {
        let _ = source.prepare_deferred();
        source.finish_deferred();
    }

    fn source_stage<S>(
        source: S,
        effects: Vec<Box<dyn AudioEffect>>,
        drain: EffectDrain,
        spec: PcmSpec,
    ) -> WarpSource<S>
    where
        S: PcmSource<Chunk = PcmChunk>,
    {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let config = kithara_warp::WarpConfig::builder().build();
            let warp = kithara_warp::Warp::new((), &config);
            let renderer = warp.renderer(spec, PcmPool::default());
            WarpSource::new(source, renderer, effects, drain, spec)
        }
        #[cfg(target_arch = "wasm32")]
        {
            WarpSource::new(source, effects, drain, spec)
        }
    }

    struct RawSource {
        chunks: VecDeque<PcmChunk>,
        head: Arc<AtomicU64>,
        seek: Arc<SeekState>,
    }

    impl PcmSource for RawSource {
        type Chunk = PcmChunk;

        fn seek_observe(&self) -> Arc<dyn SeekObserve> {
            Arc::clone(&self.seek) as Arc<dyn SeekObserve>
        }

        fn step_track(&mut self) -> TrackStep<PcmChunk> {
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
            TrackStep::Produced(Fetch::data(chunk, 0))
        }
    }

    struct BufferThenHalveFrames {
        buffered: bool,
    }

    impl AudioEffect for BufferThenHalveFrames {
        fn flush(&mut self) -> Option<PcmChunk> {
            None
        }

        fn process(&mut self, mut chunk: PcmChunk) -> Option<PcmChunk> {
            if !self.buffered {
                self.buffered = true;
                return None;
            }
            let frames = chunk.meta.frames / 2;
            let samples = usize::try_from(frames)
                .ok()?
                .checked_mul(usize::from(chunk.meta.spec.channels))?;
            chunk.samples.truncate(samples);
            chunk.meta.frames = frames;
            chunk.meta.end_timestamp = duration_for_frames(
                chunk.meta.spec.sample_rate.get(),
                chunk.meta.frame_offset.saturating_add(u64::from(frames)),
            );
            Some(chunk)
        }

        fn reset(&mut self) {
            self.buffered = false;
        }
    }

    struct DeferredSource {
        log: Arc<Mutex<Vec<&'static str>>>,
        seek: Arc<SeekState>,
        spec: PcmSpec,
    }

    impl PcmSource for DeferredSource {
        type Chunk = PcmChunk;

        fn prepare_deferred(&mut self) -> Option<PcmSpec> {
            self.log.lock().push("source.prepare");
            Some(self.spec)
        }

        fn finish_deferred(&mut self) {
            self.log.lock().push("source.finish");
        }

        fn seek_observe(&self) -> Arc<dyn SeekObserve> {
            Arc::clone(&self.seek) as Arc<dyn SeekObserve>
        }

        fn step_track(&mut self) -> TrackStep<PcmChunk> {
            TrackStep::Blocked(WaitingReason::Waiting)
        }
    }

    struct DeferredEffect {
        log: Arc<Mutex<Vec<&'static str>>>,
        serviced: Arc<Mutex<Option<PcmSpec>>>,
    }

    impl AudioEffect for DeferredEffect {
        fn service_deferred(&mut self, spec: PcmSpec) {
            self.log.lock().push("effect.service");
            *self.serviced.lock() = Some(spec);
        }

        fn flush(&mut self) -> Option<PcmChunk> {
            None
        }

        fn process(&mut self, chunk: PcmChunk) -> Option<PcmChunk> {
            Some(chunk)
        }

        fn reset(&mut self) {}
    }

    struct RevisionSource {
        discontinuity: Arc<Mutex<SourceDiscontinuity>>,
        seek: Arc<SeekState>,
    }

    impl PcmSource for RevisionSource {
        type Chunk = PcmChunk;

        fn discontinuity(&self) -> Option<SourceDiscontinuity> {
            Some(*self.discontinuity.lock())
        }

        fn seek_observe(&self) -> Arc<dyn SeekObserve> {
            Arc::clone(&self.seek) as Arc<dyn SeekObserve>
        }

        fn step_track(&mut self) -> TrackStep<PcmChunk> {
            TrackStep::Blocked(WaitingReason::Waiting)
        }
    }

    struct ResetCounter {
        resets: Arc<AtomicU64>,
    }

    impl AudioEffect for ResetCounter {
        fn flush(&mut self) -> Option<PcmChunk> {
            None
        }

        fn process(&mut self, chunk: PcmChunk) -> Option<PcmChunk> {
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

    impl PcmSource for CountingEofSource {
        type Chunk = PcmChunk;

        fn discontinuity(&self) -> Option<SourceDiscontinuity> {
            *self.discontinuity.lock()
        }

        fn seek_observe(&self) -> Arc<dyn SeekObserve> {
            Arc::clone(&self.seek) as Arc<dyn SeekObserve>
        }

        fn step_track(&mut self) -> TrackStep<PcmChunk> {
            self.steps.fetch_add(1, Ordering::AcqRel);
            TrackStep::Eof
        }
    }

    struct CountingEmptyTail {
        flushes: Arc<AtomicU64>,
        resets: Arc<AtomicU64>,
    }

    impl AudioEffect for CountingEmptyTail {
        fn flush(&mut self) -> Option<PcmChunk> {
            self.flushes.fetch_add(1, Ordering::AcqRel);
            None
        }

        fn process(&mut self, chunk: PcmChunk) -> Option<PcmChunk> {
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
        spec: PcmSpec,
    }

    impl PcmSource for SeekApplyingSource {
        type Chunk = PcmChunk;

        fn decode_epoch(&self) -> u64 {
            self.decode_epoch
        }

        fn discontinuity(&self) -> Option<SourceDiscontinuity> {
            Some(SourceDiscontinuity::new(self.revision, self.spec))
        }

        fn seek_observe(&self) -> Arc<dyn SeekObserve> {
            Arc::clone(&self.seek) as Arc<dyn SeekObserve>
        }

        fn step_track(&mut self) -> TrackStep<PcmChunk> {
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
        tail: Option<PcmChunk>,
    }

    impl AudioEffect for ResettingTail {
        fn flush(&mut self) -> Option<PcmChunk> {
            self.tail.take()
        }

        fn process(&mut self, chunk: PcmChunk) -> Option<PcmChunk> {
            Some(chunk)
        }

        fn reset(&mut self) {
            self.tail = None;
            self.resets.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn chunk(pool: &PcmPool, spec: PcmSpec, frame_offset: u64) -> PcmChunk {
        const FRAMES: usize = 128;
        PcmChunk::new(
            PcmMeta {
                spec,
                frames: 128,
                frame_offset,
                timestamp: duration_for_frames(spec.sample_rate.get(), frame_offset),
                end_timestamp: duration_for_frames(
                    spec.sample_rate.get(),
                    frame_offset.saturating_add(128),
                ),
                ..Default::default()
            },
            pool.attach(vec![0.25; FRAMES * usize::from(spec.channels)]),
        )
    }

    #[kithara::test]
    fn buffering_effect_yields_after_each_source_transition() {
        let spec = PcmSpec::new(2, NonZeroU32::new(44_100).expect("test sample rate"));
        let pool = PcmPool::default();
        let head = Arc::new(AtomicU64::new(0));
        let source = RawSource {
            chunks: VecDeque::from([
                chunk(&pool, spec, 0),
                chunk(&pool, spec, u64::from(128_u32)),
            ]),
            head: Arc::clone(&head),
            seek: Arc::new(SeekState::new()),
        };
        let effects: Vec<Box<dyn AudioEffect>> =
            vec![Box::new(BufferThenHalveFrames { buffered: false })];
        let drain = EffectDrain::new(effects.len(), &BytePool::default());
        let mut source = source_stage(source, effects, drain, spec);

        assert!(matches!(source.step_track(), TrackStep::StateChanged));
        assert_eq!(
            head.load(Ordering::Acquire),
            128,
            "one worker pass advances exactly one source transition"
        );

        let TrackStep::Produced(Fetch::Data { data, .. }) = source.step_track() else {
            panic!("the second raw chunk must pass the buffering effect");
        };

        assert_eq!(head.load(Ordering::Acquire), 256);
        assert_eq!(data.meta.frame_offset, 128);
        assert_eq!(data.meta.frames, 64);
    }

    #[kithara::test]
    fn deferred_shell_services_effects_between_source_phases() {
        let spec = PcmSpec::new(2, NonZeroU32::new(48_000).expect("test sample rate"));
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
        let initial = PcmSpec::new(2, NonZeroU32::new(44_100).expect("initial rate"));
        let changed = PcmSpec::new(1, NonZeroU32::new(48_000).expect("changed rate"));
        let discontinuity = Arc::new(Mutex::new(SourceDiscontinuity::new(7, initial)));
        let resets = Arc::new(AtomicU64::new(0));
        let source = RevisionSource {
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
    fn seek_cancels_stale_tail_and_resets_effects_once() {
        let spec = PcmSpec::new(2, NonZeroU32::new(44_100).expect("test sample rate"));
        let pool = PcmPool::default();
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
        let spec = PcmSpec::new(2, NonZeroU32::new(44_100).expect("test sample rate"));
        let pool = PcmPool::default();
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
            let TrackStep::Produced(Fetch::Data { data, .. }) = source.step_track() else {
                panic!("effect tail must be emitted before EOF");
            };
            assert_eq!(data.meta.frame_offset, expected);
        }
        assert!(matches!(source.step_track(), TrackStep::Eof));
    }

    #[kithara::test]
    fn exhausted_drain_stays_terminal_for_the_decode_epoch() {
        let spec = PcmSpec::new(2, NonZeroU32::new(44_100).expect("test sample rate"));
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
