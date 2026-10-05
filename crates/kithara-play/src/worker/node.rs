use kithara_audio::{
    AudioEvent, AudioLaneEvent, AudioSource, Fetch, PreloadGate, PreparedAudioLane, ProducerPort,
    TrackStep, WaitingReason,
};
use kithara_events::DeferredBus;
use kithara_platform::{
    sync::Arc,
    time::{Duration, Instant, WallInstant},
};
use kithara_signal::AudioChunk;
use kithara_stream::{PlayheadWrite, SeekObserve};
use kithara_test_utils::kithara;
use kithara_worker::{Task, TickResult};

use super::{EngineLoad, ReadinessProbe};

/// Per-tick state of a [`DecoderNode`].
#[derive(Default)]
#[non_exhaustive]
pub(in crate::worker) struct DecoderRuntime {
    pub(in crate::worker) last_buffer_health_emit: Option<Instant>,
    pub(in crate::worker) last_engine_load_emit: Option<Instant>,
    pub(in crate::worker) eof_sent: bool,
    pub(in crate::worker) preloaded: bool,
    pub(in crate::worker) seek_epoch: u64,
    pub(in crate::worker) chunks_sent: usize,
}

/// Play-owned node that drives one still-concrete audio source.
pub(crate) struct DecoderNode<S> {
    emit: Arc<DeferredBus<AudioLaneEvent>>,
    playhead: Arc<dyn PlayheadWrite>,
    preload_gate: Arc<PreloadGate>,
    seek_obs: Arc<dyn SeekObserve>,
    runtime: DecoderRuntime,
    engine_load: Option<Arc<EngineLoad>>,
    port: ProducerPort,
    /// Proof a staged lane owes its preparation; `None` for a playing lane.
    readiness: Option<ReadinessProbe>,
    source: S,
    preload_chunks: usize,
}

impl<S> DecoderNode<S> {
    const BUFFER_HEALTH_EMIT_MIN: Duration = Duration::from_millis(250);
    const ENGINE_LOAD_EMIT_MIN: Duration = Duration::from_millis(500);

    /// Open the one-shot preload latch that resource construction waits on.
    ///
    /// Openers are the chunk count, a terminal step (EOF, failure, cancel),
    /// and an upstream park that has audio behind it. The park belongs here
    /// because the latch gates on the decoder, not on the network: once the
    /// producer is waiting for bytes, every chunk the delivered data can
    /// yield has been yielded, and how many that is depends on where the
    /// demuxer's buffered read lands relative to the delivered segment's end.
    /// Keeping the latch shut would make construction — which owns no
    /// deadline — wait for a fetch the loader already owns and already bounds.
    fn complete_preload(&mut self) {
        if !self.runtime.preloaded {
            self.preload_gate.signal_epoch(self.runtime.seek_epoch);
            self.runtime.preloaded = true;
        }
    }

    /// Open the latch for a producer that parked upstream with audio already
    /// emitted.
    ///
    /// A park says the delivered bytes are spent, which only releases
    /// construction when they yielded something: with nothing emitted the
    /// statement is vacuous, and opening on it starts playback on a ring that
    /// holds no audio, where the playhead cannot advance past what the first
    /// fetch happened to deliver. That case stays shut and waits for the
    /// quota, a terminal step, or the next chunk.
    fn complete_preload_on_park(&mut self) {
        if self.runtime.chunks_sent > 0 {
            self.complete_preload();
        }
    }

    fn mark_preload_progress(&mut self) {
        if self.runtime.preloaded {
            return;
        }

        self.runtime.chunks_sent += 1;
        if self.runtime.chunks_sent >= self.preload_chunks {
            self.complete_preload();
        }
    }

    fn maybe_emit_buffer_health(&mut self, now: Instant) {
        if self
            .runtime
            .last_buffer_health_emit
            .is_some_and(|last| now.duration_since(last) < Self::BUFFER_HEALTH_EMIT_MIN)
        {
            return;
        }
        self.runtime.last_buffer_health_emit = Some(now);
        let position = self.playhead.position();
        let decoded_frontier = self.playhead.decoded_frontier();
        let decoded_frontier_ms = decoded_frontier.as_millis().try_into().unwrap_or(u64::MAX);
        let buffered_ms = decoded_frontier
            .saturating_sub(position)
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        self.emit.enqueue(AudioEvent::BufferHealth {
            buffered_ms,
            decoded_frontier_ms,
            seek_epoch: self.runtime.seek_epoch,
        });
    }

    fn maybe_emit_engine_load(&mut self, now: Instant) {
        let Some(load) = self.engine_load.as_ref() else {
            return;
        };
        if self
            .runtime
            .last_engine_load_emit
            .is_some_and(|last| now.duration_since(last) < Self::ENGINE_LOAD_EMIT_MIN)
        {
            return;
        }
        self.runtime.last_engine_load_emit = Some(now);
        let snapshot = load.snapshot();
        self.emit.enqueue(AudioEvent::EngineLoad {
            load: snapshot.load(),
            ms_per_chunk: snapshot.ms(),
            realtime_factor: snapshot.realtime(),
        });
    }

    fn maybe_emit_worker_telemetry(&mut self, now: Instant) {
        self.maybe_emit_buffer_health(now);
        self.maybe_emit_engine_load(now);
    }

    fn record_load(&self, busy: Duration, fetch: &Fetch<AudioChunk>) {
        if let (Some(load), Fetch::Data { data, .. }) = (self.engine_load.as_ref(), fetch) {
            load.record(busy, data.frames(), data.spec().sample_rate.get());
        }
    }
}

impl<S> DecoderNode<S>
where
    S: AudioSource<Chunk = AudioChunk>,
{
    fn sync_seek_epoch(&mut self) {
        if !self.seek_obs.take_decoder_seek() {
            return;
        }
        let current = self.seek_obs.epoch();
        if current == self.runtime.seek_epoch {
            return;
        }

        self.preload_gate.rearm();
        self.runtime = DecoderRuntime {
            seek_epoch: current,
            ..Default::default()
        };
    }
}

impl<S> DecoderNode<S>
where
    S: AudioSource<Chunk = AudioChunk>,
{
    pub(in crate::worker) fn new(
        lane: PreparedAudioLane<S>,
        engine_load: Option<Arc<EngineLoad>>,
        mut readiness: Option<ReadinessProbe>,
    ) -> Self {
        let seek_obs = lane.source.seek_observe();
        let seek_epoch = seek_obs.epoch();
        if let Some(probe) = readiness.as_mut() {
            probe.bind(seek_epoch);
        }
        Self {
            seek_obs,
            engine_load,
            readiness,
            source: lane.source,
            port: lane.port,
            playhead: lane.playhead,
            emit: lane.emit,
            preload_gate: lane.preload_gate,
            preload_chunks: lane.preload_chunks,
            runtime: DecoderRuntime {
                seek_epoch,
                ..Default::default()
            },
        }
    }
}

impl<S> Task for DecoderNode<S>
where
    S: AudioSource<Chunk = AudioChunk>,
{
    fn on_cancel(&mut self) {
        self.complete_preload();
        if let Some(probe) = self.readiness.as_mut() {
            probe.abandon();
        }
    }

    fn recycle(&mut self) {
        self.port.recycle();
        let _ = self.source.prepare_deferred();
        self.source.finish_deferred();
        self.port.flush_wake();
        if let Some(probe) = self.readiness.as_mut() {
            probe.publish();
        }
    }

    /// Distinct probes separate ring-full backpressure, which a reader can release,
    /// from a queued terminal marker, whose Backpressured result will never clear.
    #[kithara::measure(label = "play.decoder.tick")]
    #[kithara::rtsan_forbid_blocking]
    fn tick(&mut self) -> TickResult {
        self.sync_seek_epoch();

        if !self.port.can_push_direct() {
            kithara::probe_event!(
                decoder_ring_full,
                epoch = self.runtime.seek_epoch,
                chunks_sent = self.runtime.chunks_sent
            );
            return TickResult::Backpressured;
        }

        if self.runtime.chunks_sent >= self.preload_chunks && !self.runtime.preloaded {
            self.complete_preload();
        }

        let start = WallInstant::now();
        let result = match self.source.step_track() {
            TrackStep::Produced(fetch) => {
                self.record_load(start.elapsed(), &fetch);
                self.runtime.eof_sent = false;
                let (admitted, source_end) = match &fetch {
                    Fetch::Data {
                        data,
                        epoch,
                        source_end,
                    } => (
                        Some((data.meta, *epoch)),
                        source_end.map(|source_end| (source_end, *epoch)),
                    ),
                    _ => (None, None),
                };
                self.port.push_direct(fetch);
                if let (Some(probe), Some((meta, epoch))) = (self.readiness.as_mut(), admitted) {
                    probe.admit(&meta, epoch, self.preload_chunks);
                }
                kithara::probe_event!(chunk_admitted, epoch = self.runtime.seek_epoch);
                if let Some((source_end, epoch)) = source_end {
                    self.source.commit_source_end(source_end, epoch);
                }
                if let Some((meta, _)) = admitted {
                    self.playhead.set_decoded_frontier(meta.end_timestamp);
                }
                self.mark_preload_progress();
                TickResult::Progress
            }

            TrackStep::StateChanged => {
                self.runtime.eof_sent = false;
                TickResult::Progress
            }

            TrackStep::Blocked(reason) => {
                self.complete_preload_on_park();
                match reason {
                    WaitingReason::WaitingDemand => TickResult::UpstreamPending,
                    WaitingReason::Waiting | WaitingReason::WaitingMetadata => TickResult::Waiting,
                }
            }

            TrackStep::Eof if self.runtime.eof_sent => {
                kithara::probe_event!(decoder_source_spent, epoch = self.runtime.seek_epoch);
                TickResult::Backpressured
            }

            TrackStep::Eof => {
                let epoch = self.source.decode_epoch();
                let marker = Fetch::eof(epoch);
                self.port.push_direct(marker);
                self.complete_preload();
                if let Some(probe) = self.readiness.as_mut() {
                    probe.fail();
                }
                self.emit
                    .enqueue(AudioEvent::EndOfStream { seek_epoch: epoch });
                self.runtime.eof_sent = true;
                TickResult::Progress
            }

            TrackStep::Failed => {
                let epoch = self.source.decode_epoch();
                let marker = Fetch::failure(epoch);
                self.port.push_direct(marker);
                self.complete_preload();
                if let Some(probe) = self.readiness.as_mut() {
                    probe.fail();
                }
                TickResult::Done
            }
        };
        self.maybe_emit_worker_telemetry(Instant::now());
        result
    }

    fn warm_up(&mut self) {
        self.source.warm_up();
    }
}

#[cfg(test)]
mod scheduler_tests {
    use kithara_audio::{
        AudioRead, AudioSource, ChunkOutcome, Fetch, PreloadGate, TrackStep, WaitingReason,
    };
    use kithara_platform::{
        CancelToken,
        sync::Arc,
        thread,
        time::{Duration, timeout as platform_timeout},
    };
    use kithara_signal::{AudioChunk, AudioChunkInfo};
    use kithara_stream::{SeekControl, SeekObserve, SeekState};
    use kithara_test_utils::kithara;
    use kithara_worker::{
        Dispatcher, DispatcherConfig, TaskConfig, TaskHandle, Worker, WorkerConfig,
    };

    use super::{tests::prepared_node, *};
    use crate::{
        test_pools::{Pools, pools, sample_buffer},
        worker::scheduler::ServiceClass,
    };

    fn empty_chunk(pools: &Pools) -> AudioChunk {
        AudioChunk::new(AudioChunkInfo::default(), sample_buffer(pools, &[]))
    }

    struct MockSource {
        seek: Arc<dyn SeekControl>,
        seek_obs: Arc<dyn SeekObserve>,
        pools: Pools,
        ready: bool,
        should_panic: bool,
        chunks_to_produce: usize,
        cursor: usize,
    }

    impl MockSource {
        fn new(pools: Pools, chunks: usize) -> Self {
            let state = Arc::new(SeekState::new());
            let seek = Arc::clone(&state) as Arc<dyn SeekControl>;
            let seek_obs = Arc::clone(&state) as Arc<dyn SeekObserve>;
            Self {
                pools,
                seek,
                seek_obs,
                chunks_to_produce: chunks,
                cursor: 0,
                ready: true,
                should_panic: false,
            }
        }
    }

    impl AudioSource for MockSource {
        type Chunk = AudioChunk;

        fn seek_observe(&self) -> Arc<dyn SeekObserve> {
            Arc::clone(&self.seek_obs)
        }

        fn step_track(&mut self) -> TrackStep<AudioChunk> {
            if self.seek_obs.is_pending() || self.seek_obs.is_flushing() {
                let epoch = self.seek_obs.epoch();
                self.seek.complete(epoch);
                self.seek.clear_pending(epoch);
                return TrackStep::StateChanged;
            }
            if !self.ready {
                return TrackStep::Blocked(WaitingReason::Waiting);
            }
            if self.should_panic {
                panic!("mock panic for testing");
            }
            if self.cursor >= self.chunks_to_produce {
                return TrackStep::Eof;
            }
            self.cursor += 1;
            TrackStep::Produced(Fetch::data(empty_chunk(&self.pools), 0))
        }
    }

    struct FailingSource {
        seek_obs: Arc<dyn SeekObserve>,
    }

    impl Default for FailingSource {
        fn default() -> Self {
            Self {
                seek_obs: Arc::new(SeekState::new()) as Arc<dyn SeekObserve>,
            }
        }
    }

    impl AudioSource for FailingSource {
        type Chunk = AudioChunk;

        fn seek_observe(&self) -> Arc<dyn SeekObserve> {
            Arc::clone(&self.seek_obs)
        }

        fn step_track(&mut self) -> TrackStep<AudioChunk> {
            TrackStep::Failed
        }
    }

    async fn make_node<S>(
        source: S,
        ringbuf_capacity: usize,
        preload_chunks: usize,
    ) -> (
        DecoderNode<S>,
        impl FnMut() -> Option<()> + Send + 'static,
        Arc<PreloadGate>,
    )
    where
        S: AudioSource<Chunk = AudioChunk>,
    {
        let seek_obs = source.seek_observe();
        let seek_epoch = seek_obs.epoch();
        let (mut node, mut audio) =
            prepared_node(source, ringbuf_capacity, preload_chunks.max(1)).await;
        node.seek_obs = seek_obs;
        node.runtime.seek_epoch = seek_epoch;
        let preload_gate = Arc::clone(&node.preload_gate);
        let pop = move || match audio.next_chunk() {
            Ok(ChunkOutcome::Chunk(_)) => Some(()),
            Ok(ChunkOutcome::Pending { .. } | ChunkOutcome::Eof { .. }) | Err(_) => None,
        };
        (node, pop, preload_gate)
    }

    struct PlaybackScheduler {
        dispatcher: Dispatcher,
        _worker: Worker,
    }

    impl PlaybackScheduler {
        fn register<S>(&self, node: DecoderNode<S>) -> Result<TaskHandle, kithara_worker::TaskError>
        where
            S: AudioSource<Chunk = AudioChunk>,
        {
            self.dispatcher.register(
                TaskConfig::new().with_priority(ServiceClass::Audible.into()),
                |_| node,
            )
        }

        fn start(name: String, cancel: CancelToken, capacity: std::num::NonZeroUsize) -> Self {
            let worker = Worker::new(WorkerConfig::new().with_cancel(cancel));
            let dispatcher = worker.dispatcher(
                DispatcherConfig::builder()
                    .name(name)
                    .capacity(capacity)
                    .observer(crate::worker::scheduler::PlaybackObserver::default())
                    .build(),
            );
            Self {
                dispatcher,
                _worker: worker,
            }
        }

        fn wake_handle(&self) -> kithara_worker::Wake {
            self.dispatcher.wake_handle()
        }
    }

    fn test_scheduler() -> PlaybackScheduler {
        PlaybackScheduler::start(
            "kithara-play-worker-test".into(),
            CancelToken::never(),
            std::num::NonZeroUsize::new(8).expect("test capacity is non-zero"),
        )
    }

    fn register<S>(handle: &PlaybackScheduler, node: DecoderNode<S>) -> TaskHandle
    where
        S: AudioSource<Chunk = AudioChunk>,
    {
        handle
            .register(node)
            .expect("test playback task must register")
    }

    #[kithara::test(tokio)]
    #[case::progress(10, 3, "preload gate must open at the threshold")]
    #[case::eof(0, 8, "early EOF must open the preload gate")]
    async fn worker_preload_gate_fires(
        #[case] chunks: usize,
        #[case] preload: usize,
        #[case] message: &str,
    ) {
        let pools = pools();
        let handle = test_scheduler();
        let (node, _pop, gate) =
            make_node(MockSource::new(pools.clone(), chunks), 32, preload).await;
        let _id = register(&handle, node);

        platform_timeout(Duration::from_secs(1), gate.wait())
            .await
            .expect(message);
        assert!(gate.is_ready());
    }

    #[kithara::test(tokio)]
    async fn worker_preload_gate_fires_on_failure() {
        let handle = test_scheduler();
        let (node, _pop, gate) = make_node(FailingSource::default(), 32, 8).await;
        let _id = register(&handle, node);

        platform_timeout(Duration::from_secs(1), gate.wait())
            .await
            .expect("decoder failure must open the preload gate");
        assert!(gate.is_ready());
    }

    #[kithara::test(tokio)]
    async fn worker_preload_gate_reopens_after_seek() {
        let pools = pools();
        let handle = test_scheduler();
        let source = MockSource::new(pools.clone(), 10);
        let seek = Arc::clone(&source.seek);
        let (node, _pop, gate) = make_node(source, 32, 1).await;
        let _id = register(&handle, node);

        platform_timeout(Duration::from_secs(1), gate.wait())
            .await
            .expect("initial preload gate must open");

        let epoch = seek.begin(Duration::from_secs(1));
        handle.wake_handle().wake();
        platform_timeout(Duration::from_secs(1), gate.wait_for_epoch(epoch))
            .await
            .expect("post-seek gate must reopen");
    }

    /// Scheduler contracts observed through the product's `chunk_admitted` and
    /// `scheduler_pass` probes.
    #[cfg(feature = "usdt")]
    mod probed {
        use kithara_test_utils::test::usdt::{ProbeEvent, Scope, scope};

        use super::*;

        impl MockSource {
            fn not_ready(pools: Pools, chunks: usize) -> Self {
                Self {
                    ready: false,
                    ..Self::new(pools, chunks)
                }
            }

            fn panicking(pools: Pools) -> Self {
                Self {
                    should_panic: true,
                    ..Self::new(pools, 100)
                }
            }
        }

        /// Source that always produces, so its node competes for every pass.
        struct EndlessSource {
            seek_obs: Arc<dyn SeekObserve>,
            /// Time each step holds the shared worker thread before producing.
            step: Duration,
            pools: Pools,
        }

        impl AudioSource for EndlessSource {
            type Chunk = AudioChunk;

            fn seek_observe(&self) -> Arc<dyn SeekObserve> {
                Arc::clone(&self.seek_obs)
            }

            fn step_track(&mut self) -> TrackStep<AudioChunk> {
                thread::sleep(self.step);
                TrackStep::Produced(Fetch::data(empty_chunk(&self.pools), 0))
            }
        }

        /// Source that never has data, so its node waits on every pass.
        struct WaitingSource {
            seek_obs: Arc<dyn SeekObserve>,
            /// Time each step holds the shared worker thread before waiting.
            step: Duration,
        }

        impl AudioSource for WaitingSource {
            type Chunk = AudioChunk;

            fn seek_observe(&self) -> Arc<dyn SeekObserve> {
                Arc::clone(&self.seek_obs)
            }

            fn step_track(&mut self) -> TrackStep<AudioChunk> {
                thread::sleep(self.step);
                TrackStep::Blocked(WaitingReason::Waiting)
            }
        }

        fn new_seek() -> Arc<dyn SeekObserve> {
            Arc::new(SeekState::new()) as Arc<dyn SeekObserve>
        }

        fn is(event: &ProbeEvent, probe: &str) -> bool {
            event.probe == probe
        }

        fn pass_field(event: &ProbeEvent, field: &str) -> u64 {
            event
                .field(field)
                .expect("scheduler_pass carries its counts")
        }

        /// Waits for a chunk admission recorded after `seen` probes.
        async fn admitted_after(trace: &Scope, handle: &PlaybackScheduler, seen: usize) {
            handle.wake_handle().wake();
            trace
                .wait_for(|events| events[seen..].iter().any(|e| is(e, "chunk_admitted")))
                .await;
        }

        /// Waits for a scheduler pass recorded after `seen` probes that satisfies `holds`.
        async fn pass_after<F>(trace: &Scope, handle: &PlaybackScheduler, seen: usize, holds: F)
        where
            F: Fn(&ProbeEvent) -> bool,
        {
            handle.wake_handle().wake();
            trace
                .wait_for(|events| {
                    events[seen..]
                        .iter()
                        .any(|e| is(e, "scheduler_pass") && holds(e))
                })
                .await;
        }

        async fn receive_chunks<P>(
            trace: &Scope,
            handle: &PlaybackScheduler,
            pop: &mut P,
            count: usize,
        ) where
            P: FnMut() -> Option<()>,
        {
            let mut received = 0;
            loop {
                let seen = trace.events().len();
                while received < count && pop().is_some() {
                    received += 1;
                }
                if received == count {
                    return;
                }
                admitted_after(trace, handle, seen).await;
            }
        }

        #[kithara::test(tokio, flash(false))]
        async fn worker_delivers_chunks() {
            let trace = scope();
            let pools = pools();
            let handle = test_scheduler();
            let (node, mut pop, _) = make_node(MockSource::new(pools.clone(), 10), 32, 3).await;
            let _id = register(&handle, node);

            receive_chunks(&trace, &handle, &mut pop, 5).await;
        }

        #[kithara::test(tokio, flash(false))]
        async fn worker_multi_track_round_robin() {
            let trace = scope();
            let pools = pools();
            let handle = test_scheduler();
            let (node_a, mut pop_a, _) = make_node(MockSource::new(pools.clone(), 10), 32, 1).await;
            let (node_b, mut pop_b, _) = make_node(MockSource::new(pools.clone(), 10), 32, 1).await;
            let _id_a = register(&handle, node_a);
            let _id_b = register(&handle, node_b);

            receive_chunks(&trace, &handle, &mut pop_a, 3).await;
            receive_chunks(&trace, &handle, &mut pop_b, 3).await;
        }

        #[kithara::test(tokio, flash(false))]
        async fn worker_skips_not_ready_tracks() {
            let trace = scope();
            let pools = pools();
            let handle = test_scheduler();
            let (node_a, mut pop_a, _) = make_node(MockSource::new(pools.clone(), 10), 32, 1).await;
            let (node_b, mut pop_b, _) =
                make_node(MockSource::not_ready(pools.clone(), 10), 32, 1).await;
            let _id_a = register(&handle, node_a);
            let _id_b = register(&handle, node_b);

            receive_chunks(&trace, &handle, &mut pop_a, 1).await;
            pass_after(&trace, &handle, 0, |pass| pass_field(pass, "waiting") >= 1).await;
            assert!(pop_b().is_none(), "not-ready track should receive nothing");
        }

        #[kithara::test(tokio, flash(false))]
        async fn worker_overflow_on_full_ringbuf() {
            let trace = scope();
            let pools = pools();
            let handle = test_scheduler();
            let (node, mut pop, _) = make_node(MockSource::new(pools.clone(), 5), 1, 1).await;
            let _id = register(&handle, node);

            receive_chunks(&trace, &handle, &mut pop, 1).await;
            let seen = trace.events().len();
            pass_after(&trace, &handle, seen, |pass| {
                pass_field(pass, "backpressured") >= 1
            })
            .await;
            receive_chunks(&trace, &handle, &mut pop, 1).await;
        }

        #[kithara::test(tokio, flash(false))]
        async fn worker_panic_isolation() {
            let trace = scope();
            let pools = pools();
            let handle = test_scheduler();
            let (node_a, _, _) = make_node(MockSource::panicking(pools.clone()), 32, 1).await;
            let (node_b, mut pop_b, _) = make_node(MockSource::new(pools.clone(), 10), 32, 1).await;
            let _id_a = register(&handle, node_a);
            let _id_b = register(&handle, node_b);

            receive_chunks(&trace, &handle, &mut pop_b, 3).await;
        }

        #[kithara::test(tokio, flash(false))]
        async fn worker_seek_enters_pending_reset() {
            let trace = scope();
            let pools = pools();
            let handle = test_scheduler();
            let source = MockSource::new(pools.clone(), 100);
            let seek = Arc::clone(&source.seek);
            let (node, mut pop, _) = make_node(source, 32, 1).await;
            let _id = register(&handle, node);

            receive_chunks(&trace, &handle, &mut pop, 2).await;
            let seen = trace.events().len();
            pass_after(&trace, &handle, seen, |pass| {
                pass_field(pass, "backpressured") >= 1
            })
            .await;
            let seen = trace.events().len();
            let epoch = seek.begin(Duration::from_secs(10));
            handle.wake_handle().wake();
            // The consumer must observe the seek and retire the full pre-seek ring.
            let _ = pop();
            trace
                .wait_for(|events| {
                    events[seen..]
                        .iter()
                        .any(|e| is(e, "chunk_admitted") && e.field("epoch") == Some(epoch))
                })
                .await;
            receive_chunks(&trace, &handle, &mut pop, 1).await;
        }

        #[kithara::test(tokio, flash(false))]
        async fn worker_unregister_removes_track() {
            let trace = scope();
            let pools = pools();
            let handle = test_scheduler();
            let (node, mut pop, _) = make_node(MockSource::new(pools.clone(), 100), 32, 1).await;
            let id = register(&handle, node);

            receive_chunks(&trace, &handle, &mut pop, 2).await;
            let seen = trace.events().len();
            drop(id);
            pass_after(&trace, &handle, seen, |pass| {
                pass_field(pass, "active") == 0
            })
            .await;
            while pop().is_some() {}
            let seen = trace.events().len();
            pass_after(&trace, &handle, seen, |_| true).await;
            assert!(pop().is_none(), "no chunks should arrive after unregister");
        }

        #[kithara::test(tokio, flash(false))]
        async fn unregister_one_task_keeps_sibling_running_and_releases_capacity() {
            let trace = scope();
            let pools = pools();
            let handle = PlaybackScheduler::start(
                "kithara-play-worker-capacity-test".into(),
                CancelToken::never(),
                std::num::NonZeroUsize::new(2).expect("test capacity is non-zero"),
            );
            let (node_a, mut pop_a, _) = make_node(MockSource::new(pools.clone(), 100), 1, 1).await;
            let (node_b, mut pop_b, _) = make_node(MockSource::new(pools.clone(), 100), 1, 1).await;

            let id_a = register(&handle, node_a);
            let id_b = register(&handle, node_b);
            receive_chunks(&trace, &handle, &mut pop_a, 1).await;
            receive_chunks(&trace, &handle, &mut pop_b, 1).await;

            drop(id_a);
            let (node_c, _, _) = make_node(MockSource::new(pools.clone(), 1), 1, 1).await;
            let id_c = handle
                .register(node_c)
                .expect("unregister must release capacity");

            while pop_b().is_some() {}
            receive_chunks(&trace, &handle, &mut pop_b, 1).await;

            drop(id_b);
            drop(id_c);
        }

        #[kithara::test(tokio, flash(false))]
        #[case::instant_step(Duration::ZERO)]
        #[case::sync_blocking_step(Duration::from_millis(10))]
        async fn shared_worker_waiting_track_does_not_starve_producing_track(
            #[case] step: Duration,
        ) {
            let trace = scope();
            let pools = pools();
            let handle = test_scheduler();
            let (node_a, mut pop_a, _) =
                make_node(MockSource::new(pools.clone(), 100), 32, 0).await;
            let _id_a = register(&handle, node_a);
            let (node_b, _pop_b, _) = make_node(
                WaitingSource {
                    step,
                    seek_obs: new_seek(),
                },
                32,
                0,
            )
            .await;
            let _id_b = register(&handle, node_b);

            pass_after(&trace, &handle, 0, |pass| pass_field(pass, "waiting") >= 1).await;
            receive_chunks(&trace, &handle, &mut pop_a, 64).await;
        }

        #[kithara::test(tokio, flash(false))]
        #[case::instant_step(Duration::ZERO)]
        #[case::sync_blocking_step(Duration::from_millis(10))]
        async fn shared_worker_endless_producer_does_not_starve_other_tracks(
            #[case] step: Duration,
        ) {
            const SOURCE_CHUNKS: usize = 1000;

            let trace = scope();
            let pools = pools();
            let handle = test_scheduler();
            let (node_a, mut pop_a, _) =
                make_node(MockSource::new(pools.clone(), SOURCE_CHUNKS), 32, 0).await;
            let _id_a = register(&handle, node_a);
            let (node_b, mut pop_b, _) = make_node(
                EndlessSource {
                    step,
                    pools: pools.clone(),
                    seek_obs: new_seek(),
                },
                32,
                0,
            )
            .await;
            let _id_b = register(&handle, node_b);

            let mut delivered = 0;
            loop {
                let seen = trace.events().len();
                while pop_a().is_some() {
                    delivered += 1;
                }
                while pop_b().is_some() {}
                if delivered == SOURCE_CHUNKS {
                    return;
                }
                admitted_after(&trace, &handle, seen).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_assets::AssetStore;
    use kithara_audio::{
        Audio, AudioConfig, AudioEvent, AudioRead, AudioSource, ChunkOutcome, Fetch,
        NoResamplerBackend, PreloadGate, SourceEnd, TrackStep, WaitingReason,
        mock::AudioSourceMock,
    };
    use kithara_command::{ChannelConfig, channel};
    use kithara_effects::EffectDrain;
    use kithara_events::{DeferredBus, EventBus};
    use kithara_platform::{
        sync::{Arc, Mutex},
        time::Duration,
    };
    use kithara_render::{LaneProtocol, WarpSource};
    use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec};
    use kithara_stream::{
        PlayheadRead, PlayheadState, PlayheadWrite, SeekControl, SeekObserve, SeekState, Stream,
        mock::NoopWorkerWake,
    };
    use kithara_test_fixtures::{assets, unit_fixtures::eq_silence as node_silence};
    use kithara_test_utils::kithara;
    use kithara_worker::{Task, TickResult};
    use unimock::{MockFn, Unimock, matching};

    use super::*;
    use crate::{
        test_pools::{Pools, pools, sample_buffer},
        worker::EngineLoad,
    };

    pub(super) async fn prepared_node<S>(
        source: S,
        capacity: usize,
        preload_chunks: usize,
    ) -> (
        DecoderNode<S>,
        Audio<Stream<kithara_file::File<crate::test_pools::TestPools>>>,
    )
    where
        S: AudioSource<Chunk = AudioChunk>,
    {
        let pools = pools();
        let path = assets::signal_wav_sine440_120ms()
            .path()
            .expect("native WAV fixture path");
        let stream =
            kithara_file::FileConfig::for_src(kithara_file::FileSrc::Local(path.to_owned()))
                .store(AssetStore::builder(pools.clone()).build())
                .pools(pools.clone())
                .build();
        let config = AudioConfig::<_, NoResamplerBackend>::for_stream(stream)
            .audio_buffer_chunks(capacity)
            .preload_chunks(
                std::num::NonZeroUsize::new(preload_chunks).expect("non-zero preload threshold"),
            )
            .build();
        let prepared = Audio::prepare(config, Arc::new(NoopWorkerWake), pools)
            .await
            .unwrap_or_else(|error| panic!("prepare real audio lane: {error}"))
            .map(|audio, _| (audio, source));
        let (audio, lane) = prepared.into();
        let node = DecoderNode {
            source: lane.source,
            port: lane.port,
            seek_obs: Arc::new(SeekState::new()) as Arc<dyn SeekObserve>,
            preload_gate: lane.preload_gate,
            playhead: lane.playhead,
            emit: lane.emit,
            preload_chunks: lane.preload_chunks,
            engine_load: None,
            readiness: None,
            runtime: DecoderRuntime::default(),
        };
        (node, audio)
    }

    fn empty_chunk(pools: &Pools) -> AudioChunk {
        AudioChunk::new(AudioChunkInfo::default(), sample_buffer(pools, &[]))
    }

    struct PersistentEofSource {
        seek: Arc<SeekState>,
    }

    struct OneChunkSource {
        seek: Arc<SeekState>,
        chunk: Option<AudioChunk>,
    }

    struct CommitSource {
        commits: Arc<Mutex<Vec<(SourceEnd, u64)>>>,
        seek: Arc<SeekState>,
        chunk: Option<AudioChunk>,
        leading_chunk: Option<AudioChunk>,
        source_end: SourceEnd,
    }

    impl AudioSource for CommitSource {
        type Chunk = AudioChunk;

        fn commit_source_end(&mut self, source_end: SourceEnd, epoch: u64) {
            self.commits.lock().push((source_end, epoch));
        }

        fn seek_observe(&self) -> Arc<dyn SeekObserve> {
            Arc::clone(&self.seek) as Arc<dyn SeekObserve>
        }

        fn step_track(&mut self) -> TrackStep<AudioChunk> {
            if let Some(chunk) = self.leading_chunk.take() {
                return TrackStep::Produced(Fetch::data(chunk, 0));
            }
            self.chunk.take().map_or(TrackStep::Eof, |chunk| {
                TrackStep::Produced(Fetch::rendered(chunk, 7, self.source_end))
            })
        }
    }

    impl AudioSource for PersistentEofSource {
        type Chunk = AudioChunk;

        fn seek_observe(&self) -> Arc<dyn SeekObserve> {
            Arc::clone(&self.seek) as Arc<dyn SeekObserve>
        }

        fn step_track(&mut self) -> TrackStep<AudioChunk> {
            TrackStep::Eof
        }
    }

    impl AudioSource for OneChunkSource {
        type Chunk = AudioChunk;

        fn seek_observe(&self) -> Arc<dyn SeekObserve> {
            Arc::clone(&self.seek) as Arc<dyn SeekObserve>
        }

        fn step_track(&mut self) -> TrackStep<AudioChunk> {
            self.chunk.take().map_or(TrackStep::Eof, |chunk| {
                TrackStep::Produced(Fetch::data(chunk, 0))
            })
        }
    }

    #[kithara::test(tokio)]
    async fn decoder_node_eof_under_backpressure() {
        let pools = pools();
        let source = OneChunkSource {
            seek: Arc::new(SeekState::new()),
            chunk: Some(empty_chunk(&pools)),
        };

        let bus = EventBus::new(8);
        let mut events = bus.subscribe();
        let (mut node, mut audio) = prepared_node(source, 1, 1).await;
        node.emit = Arc::new(DeferredBus::new(bus, 8));

        assert_eq!(node.tick(), TickResult::Progress);
        assert_eq!(node.tick(), TickResult::Backpressured);
        assert!(!node.runtime.eof_sent);

        assert!(matches!(audio.next_chunk(), Ok(ChunkOutcome::Chunk(_))));

        assert_eq!(node.tick(), TickResult::Progress);
        assert!(node.runtime.eof_sent);
        assert!(matches!(audio.next_chunk(), Ok(ChunkOutcome::Eof { .. })));
        assert_eq!(node.tick(), TickResult::Backpressured);

        node.emit.flush();
        let end_events = std::iter::from_fn(|| events.try_recv().ok())
            .filter(|envelope| matches!(envelope.event, AudioEvent::EndOfStream { .. }))
            .count();
        assert_eq!(end_events, 1, "current-epoch EOF must publish exactly once");
    }

    #[kithara::test(tokio)]
    async fn decoder_node_does_not_republish_exhausted_warp_source_eof() {
        let pools = pools();
        let seek = Arc::new(SeekState::new());
        let source = PersistentEofSource {
            seek: Arc::clone(&seek),
        };
        let effects = Vec::new();
        let drain = EffectDrain::new(effects.len(), &pools)
            .unwrap_or_else(|error| panic!("test effect drain: {error}"));
        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("test sample rate"));
        let config = kithara_warp::WarpConfig::builder().build();
        let warp = kithara_warp::Warp::new((), &config);
        let renderer = warp.renderer(spec, pools.clone());
        let (_lane, inbox) = channel::<LaneProtocol>(ChannelConfig::builder().build());
        let source = WarpSource::new(source, renderer, effects, drain, spec, pools, inbox);
        let bus = EventBus::new(8);
        let mut events = bus.subscribe();
        let (mut node, mut audio) = prepared_node(source, 1, 1).await;
        node.emit = Arc::new(DeferredBus::new(bus, 8));

        assert_eq!(node.tick(), TickResult::Progress);
        assert_eq!(node.tick(), TickResult::Progress);
        assert!(matches!(audio.next_chunk(), Ok(ChunkOutcome::Eof { .. })));
        assert_eq!(node.tick(), TickResult::Backpressured);
        assert_eq!(node.tick(), TickResult::Backpressured);

        node.emit.flush();
        let end_events = std::iter::from_fn(|| events.try_recv().ok())
            .filter(|envelope| matches!(envelope.event, AudioEvent::EndOfStream { .. }))
            .count();
        assert_eq!(end_events, 1);
    }

    #[kithara::test(tokio)]
    async fn decoder_node_records_engine_load_on_produced(node_silence: Vec<f32>) {
        let pools = pools();
        use std::num::NonZero;

        use kithara_signal::AudioSpec;

        let meter = Arc::new(EngineLoad::default());
        assert!(!meter.snapshot().is_active(), "idle before any tick");

        let chunk = AudioChunk::new(
            AudioChunkInfo {
                spec: AudioSpec {
                    channels: 2,
                    sample_rate: NonZero::new(44_100).unwrap(),
                },
                frames: 4_410,
                ..Default::default()
            },
            sample_buffer(&pools, &node_silence),
        );
        let source = Unimock::new(
            AudioSourceMock::step_track
                .next_call(matching!())
                .returns(TrackStep::Produced(Fetch::data(chunk, 0))),
        );

        let (mut node, _audio) = prepared_node(source, 4, 1).await;
        node.engine_load = Some(Arc::clone(&meter));

        assert_eq!(node.tick(), TickResult::Progress);
        assert!(
            meter.snapshot().is_active(),
            "engine meter records on a Produced tick: {:?}",
            meter.snapshot()
        );
    }

    #[kithara::test(tokio)]
    async fn worker_telemetry_throttles_immediate_repeats() {
        let source = Unimock::new(());
        let gate = Arc::new(PreloadGate::default());
        let seek = Arc::new(SeekState::new());
        let playhead = Arc::new(PlayheadState::new());
        playhead.set_position(Duration::from_millis(100));
        playhead.set_decoded_frontier(Duration::from_millis(350));
        let bus = EventBus::new(8);
        let mut events = bus.subscribe();
        let emit = Arc::new(DeferredBus::new(bus, 8));
        let meter = Arc::new(EngineLoad::default());
        meter.record(Duration::from_millis(5), 4_410, 44_100);

        let (mut node, _audio) = prepared_node(source, 4, 1).await;
        node.seek_obs = Arc::clone(&seek) as Arc<dyn SeekObserve>;
        node.preload_gate = gate;
        node.playhead = Arc::clone(&playhead) as Arc<dyn PlayheadWrite>;
        node.emit = Arc::clone(&emit);
        node.engine_load = Some(meter);

        let now = Instant::now();
        node.maybe_emit_worker_telemetry(now);
        node.maybe_emit_worker_telemetry(now);
        emit.flush();

        assert!(matches!(
            events.try_recv().map(|envelope| envelope.event),
            Ok(AudioEvent::BufferHealth {
                buffered_ms: 250,
                decoded_frontier_ms: 350,
                seek_epoch: 0,
            })
        ));
        assert!(matches!(
            events.try_recv().map(|envelope| envelope.event),
            Ok(AudioEvent::EngineLoad { .. })
        ));
        assert!(
            events.try_recv().is_err(),
            "second immediate tick stays throttled"
        );
    }

    #[kithara::test(tokio)]
    async fn decoder_node_distinguishes_failed_from_eof_on_the_wire() {
        let eof_source = Unimock::new((
            AudioSourceMock::step_track
                .next_call(matching!())
                .returns(TrackStep::Eof),
            AudioSourceMock::decode_epoch.stub(|each| {
                each.call(matching!()).returns(0u64);
            }),
        ));
        let (mut eof_node, mut eof_audio) = prepared_node(eof_source, 1, 1).await;
        assert_eq!(eof_node.tick(), TickResult::Progress);
        let eof_marker = eof_audio.next_chunk();

        let failed_source = Unimock::new((
            AudioSourceMock::step_track
                .next_call(matching!())
                .returns(TrackStep::Failed),
            AudioSourceMock::decode_epoch.stub(|each| {
                each.call(matching!()).returns(0u64);
            }),
        ));
        let (mut failed_node, mut failed_audio) = prepared_node(failed_source, 1, 1).await;
        let _ = failed_node.tick();
        let failed_marker = failed_audio.next_chunk();

        assert!(matches!(eof_marker, Ok(ChunkOutcome::Eof { .. })));
        assert!(failed_marker.is_err());
    }

    #[kithara::test(tokio)]
    async fn deferred_eof_event_keeps_the_decode_epoch() {
        let seek_state = Arc::new(SeekState::new());
        let seek_obs = Arc::clone(&seek_state) as Arc<dyn SeekObserve>;

        let source = Unimock::new((
            AudioSourceMock::step_track
                .next_call(matching!())
                .returns(TrackStep::Eof),
            AudioSourceMock::decode_epoch
                .next_call(matching!())
                .returns(0u64),
        ));

        let bus = EventBus::new(8);
        let mut events = bus.subscribe();
        let (mut node, _audio) = prepared_node(source, 1, 1).await;
        node.seek_obs = seek_obs;
        node.emit = Arc::new(DeferredBus::new(bus, 8));
        assert_eq!(node.tick(), TickResult::Progress);

        let live_epoch = seek_state.begin(Duration::from_secs(1));
        assert_eq!(live_epoch, 1, "seek overtakes the deferred EOF flush");

        node.emit.flush();
        let mut eof_epochs = std::iter::from_fn(|| events.try_recv().ok()).filter_map(|envelope| {
            match envelope.event {
                AudioEvent::EndOfStream { seek_epoch } => Some(seek_epoch),
                AudioEvent::FormatDetected { .. }
                | AudioEvent::FormatChanged { .. }
                | AudioEvent::PlaybackProgress { .. }
                | AudioEvent::OutputAvailable
                | AudioEvent::SeekLifecycle { .. }
                | AudioEvent::SeekComplete { .. }
                | AudioEvent::SeekRejected { .. }
                | AudioEvent::DecoderReady { .. }
                | AudioEvent::TrackFailed { .. }
                | AudioEvent::UnderrunStarted { .. }
                | AudioEvent::UnderrunEnded { .. }
                | AudioEvent::BufferHealth { .. }
                | AudioEvent::EngineLoad { .. }
                | AudioEvent::PlaybackResamplerConfigured { .. } => None,
            }
        });
        assert_eq!(eof_epochs.next(), Some(0));
        assert_eq!(eof_epochs.next(), None);
    }

    #[kithara::test(tokio)]
    async fn decoded_frontier_advances_only_after_final_port_admission() {
        let pools = pools();
        let end = Duration::from_millis(750);
        let mut chunk = empty_chunk(&pools);
        chunk.meta.end_timestamp = end;
        let source = Unimock::new((
            AudioSourceMock::step_track
                .next_call(matching!())
                .returns(TrackStep::Produced(Fetch::data(empty_chunk(&pools), 0))),
            AudioSourceMock::step_track
                .next_call(matching!())
                .returns(TrackStep::Produced(Fetch::data(chunk, 0))),
        ));
        let playhead = Arc::new(PlayheadState::new());
        let (mut node, mut audio) = prepared_node(source, 1, 1).await;
        node.playhead = Arc::clone(&playhead) as Arc<dyn PlayheadWrite>;

        assert_eq!(node.tick(), TickResult::Progress);
        assert_eq!(node.tick(), TickResult::Backpressured);
        assert_eq!(playhead.decoded_frontier(), Duration::ZERO);

        assert!(matches!(audio.next_chunk(), Ok(ChunkOutcome::Chunk(_))));
        assert_eq!(node.tick(), TickResult::Progress);
        assert_eq!(playhead.decoded_frontier(), end);
    }

    #[kithara::test(tokio)]
    async fn source_end_commits_only_after_final_port_admission() {
        let pools = pools();
        let source_end = SourceEnd::new(
            12_345,
            NonZeroU32::new(44_100).expect("test sample rate is non-zero"),
        );
        let commits = Arc::new(Mutex::new(Vec::new()));
        let source = CommitSource {
            source_end,
            leading_chunk: Some(empty_chunk(&pools)),
            chunk: Some(empty_chunk(&pools)),
            commits: Arc::clone(&commits),
            seek: Arc::new(SeekState::new()),
        };
        let (mut node, mut audio) = prepared_node(source, 1, 1).await;

        assert_eq!(node.tick(), TickResult::Progress);
        assert_eq!(node.tick(), TickResult::Backpressured);
        assert!(commits.lock().is_empty());

        assert!(matches!(audio.next_chunk(), Ok(ChunkOutcome::Chunk(_))));
        assert_eq!(node.tick(), TickResult::Progress);
        assert_eq!(commits.lock().as_slice(), &[(source_end, 7)]);
    }

    #[kithara::test(tokio)]
    async fn decoder_node_live_upstream_demand_does_not_tick_hang_wait() {
        let source = Unimock::new(
            AudioSourceMock::step_track
                .next_call(matching!())
                .returns(TrackStep::Blocked(WaitingReason::WaitingDemand)),
        );

        let (mut node, _audio) = prepared_node(source, 2, 1).await;

        assert_eq!(node.tick(), TickResult::UpstreamPending);
    }

    /// A producer that parked upstream with audio behind it opens the preload
    /// latch. The chunk count reachable before the demuxer reads past the
    /// delivered segment is not a property the pipeline controls, so a latch that
    /// only counts chunks leaves resource construction waiting on a fetch that may
    /// not land. The quota here is two against one emitted chunk, so the count
    /// cannot be what opens it.
    #[kithara::test(tokio)]
    #[case(WaitingReason::Waiting)]
    #[case(WaitingReason::WaitingDemand)]
    #[case(WaitingReason::WaitingMetadata)]
    async fn decoder_node_upstream_park_after_audio_opens_the_preload_gate(
        #[case] reason: WaitingReason,
    ) {
        let pools = pools();
        let source = Unimock::new((
            AudioSourceMock::step_track
                .next_call(matching!())
                .returns(TrackStep::Produced(Fetch::data(empty_chunk(&pools), 0))),
            AudioSourceMock::step_track
                .next_call(matching!())
                .returns(TrackStep::Blocked(reason)),
        ));

        let (mut node, _audio) = prepared_node(source, 4, 2).await;
        let gate = Arc::clone(&node.preload_gate);

        let _ = node.tick();
        assert!(
            !gate.is_ready(),
            "the chunk quota is not met after one chunk"
        );

        let _ = node.tick();

        assert!(
            gate.is_ready(),
            "a producer parked on {reason:?} with audio behind it must not strand the construction wait"
        );
    }

    /// A park with nothing emitted leaves the latch shut.
    ///
    /// The park states that the delivered bytes are spent, which releases
    /// construction only when they yielded something. With no chunk behind it the
    /// statement is vacuous, and opening on it starts playback against a ring that
    /// holds no audio, where the playhead cannot advance past whatever the first
    /// fetch happened to deliver.
    #[kithara::test(tokio)]
    #[case(WaitingReason::Waiting)]
    #[case(WaitingReason::WaitingDemand)]
    #[case(WaitingReason::WaitingMetadata)]
    async fn decoder_node_park_without_audio_keeps_the_preload_gate_shut(
        #[case] reason: WaitingReason,
    ) {
        let source = Unimock::new(
            AudioSourceMock::step_track
                .next_call(matching!())
                .returns(TrackStep::Blocked(reason)),
        );

        let (mut node, _audio) = prepared_node(source, 2, 1).await;
        let gate = Arc::clone(&node.preload_gate);

        let _ = node.tick();

        assert!(
            !gate.is_ready(),
            "a park on {reason:?} with nothing emitted is not preload"
        );
    }

    /// The park opener does not weaken the count opener: a producer that keeps
    /// delivering never reaches a park, so the latch still waits for the full
    /// chunk quota.
    #[kithara::test(tokio)]
    async fn decoder_node_preload_gate_stays_shut_below_the_chunk_quota() {
        let pools = pools();
        let source = Unimock::new(
            AudioSourceMock::step_track
                .next_call(matching!())
                .returns(TrackStep::Produced(Fetch::data(empty_chunk(&pools), 0))),
        );

        let (mut node, _audio) = prepared_node(source, 4, 2).await;
        let gate = Arc::clone(&node.preload_gate);

        let _ = node.tick();

        assert!(
            !gate.is_ready(),
            "one chunk of a two-chunk quota is not preload"
        );
    }

    #[kithara::test(tokio)]
    async fn decoder_node_seek_rearms_preload_gate() {
        let pools = pools();
        let seek_state = Arc::new(SeekState::new());
        let source = Unimock::new((
            AudioSourceMock::step_track
                .next_call(matching!())
                .returns(TrackStep::Produced(Fetch::data(empty_chunk(&pools), 0))),
            AudioSourceMock::step_track
                .next_call(matching!())
                .returns(TrackStep::StateChanged),
            AudioSourceMock::step_track
                .next_call(matching!())
                .returns(TrackStep::Produced(Fetch::data(empty_chunk(&pools), 0))),
        ));

        let (mut node, mut audio) = prepared_node(source, 1, 1).await;
        let gate = Arc::clone(&node.preload_gate);
        node.seek_obs = Arc::clone(&seek_state) as Arc<dyn SeekObserve>;

        assert_eq!(node.tick(), TickResult::Progress);
        assert!(node.runtime.preloaded);
        assert!(gate.is_ready(), "first chunk opens the gate");

        let epoch = SeekControl::begin(&*seek_state, Duration::from_secs(1));

        assert_eq!(node.tick(), TickResult::Backpressured);
        assert!(!node.runtime.preloaded, "seek resets the preload runtime");
        assert!(!gate.is_ready(), "sync_seek_epoch closes the gate");

        assert!(
            matches!(audio.next_chunk(), Ok(ChunkOutcome::Chunk(_))),
            "consumer discards the stale pre-seek chunk"
        );

        assert_eq!(node.tick(), TickResult::Progress);
        assert!(
            !node.runtime.preloaded,
            "source first applies the seek epoch"
        );

        assert_eq!(node.tick(), TickResult::Progress);
        assert!(node.runtime.preloaded);
        assert!(gate.is_ready(), "post-seek refill reopens the gate");
        assert!(
            gate.is_ready_for_epoch(epoch),
            "post-seek refill must open the new seek epoch"
        );
    }
}
