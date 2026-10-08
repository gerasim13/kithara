use std::{
    future::poll_fn,
    task::{Context, Poll},
};

use kithara_audio::{
    AudioSource, Fetch, SourceEnd, TrackStep, WaitingReason, TrackFailureKind,
};
use kithara_bufpool::{HasPool, PoolRegion};
use kithara_platform::{sync::Arc, time::WallInstant};
use kithara_signal::{AudioChunk, AudioChunkInfo, FrameCount, SegmentId};
use kithara_stream::ActivityWriter;
use kithara_test_utils::kithara;
use kithara_worker::{Priority, Task, TickResult};
use ringbuf::traits::{Consumer, Observer, Producer};

use super::{EngineLoad, PcmPacket, reader::PcmProducer};
use crate::{ServiceClass, WarpSource, dispatcher::LaneTask};

struct PendingPacket {
    packet: PcmPacket,
    source_end: Option<SourceEnd>,
}

/// Worker-owned source, rendering, ring producer and transport publisher.
pub struct DecoderNode<T, S> {
    source: WarpSource<T, S>,
    port: PcmProducer,
    activity: Option<ActivityWriter>,
    priority: ServiceClass,
    pending: Option<PendingPacket>,
    terminal: Option<Result<SegmentId, TrackFailureKind>>,
    load_error: Option<TrackFailureKind>,
    last_output: AudioChunkInfo,
    engine_load: Option<Arc<EngineLoad>>,
    pools: PoolRegion<S>,
}

impl<T, S> DecoderNode<T, S>
where
    T: AudioSource<Chunk = AudioChunk>,
    S: HasPool<f32> + Send + Sync + 'static,
{
    pub(super) fn new(
        source: WarpSource<T, S>,
        port: PcmProducer,
        activity: Option<ActivityWriter>,
        initial: AudioChunkInfo,
        engine_load: Option<Arc<EngineLoad>>,
        pools: PoolRegion<S>,
    ) -> Self {
        Self {
            source,
            port,
            activity,
            priority: ServiceClass::Warm,
            pending: None,
            terminal: None,
            load_error: None,
            last_output: initial,
            engine_load,
            pools,
        }
    }

    pub(super) async fn preload(&mut self) -> Result<(), TrackFailureKind> {
        self.warm_up();
        poll_fn(|cx| {
            let _ = self.source.poll_commands(cx);
            self.recycle();
            let result = self.tick();
            if let Some(error) = self.load_error.take() {
                return Poll::Ready(Err(error));
            }
            if self.source.is_preloaded() || self.terminal.is_some() {
                return Poll::Ready(Ok(()));
            }
            if result == TickResult::Progress {
                cx.waker().wake_by_ref();
            }
            Poll::Pending
        })
        .await
    }

    pub(super) fn engine_latency(&self) -> FrameCount {
        self.source.engine_latency()
    }

    fn synchronize(&mut self) -> Result<(), TrackFailureKind> {
        self.source.service_commands()?;
        let segment = self.source.cursor().segment;
        if self.terminal.is_some_and(|terminal| matches!(terminal, Ok(ended) if ended != segment)) {
            self.terminal = None;
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| match &pending.packet {
                PcmPacket::Chunk(chunk) => chunk.meta.segment != segment,
                PcmPacket::Failed { .. } => false,
            })
            && let Some(pending) = self.pending.take()
        {
            self.retire(pending.packet);
        }
        Ok(())
    }

    fn retire(&self, packet: PcmPacket) {
        if let PcmPacket::Chunk(chunk) = packet {
            self.source.retire_chunk(chunk);
        }
    }

    fn fail(&mut self, failure: TrackFailureKind) {
        if matches!(self.terminal, Some(Err(_)))
            || self.pending.as_ref().is_some_and(|pending| matches!(pending.packet, PcmPacket::Failed { .. }))
        {
            return;
        }
        self.load_error = Some(failure);
        self.pending = Some(PendingPacket {
            packet: PcmPacket::Failed {
                segment: self.source.cursor().segment,
                failure,
            },
            source_end: None,
        });
    }

    fn admit(&mut self) -> TickResult {
        let Some(pending) = self.pending.take() else {
            return TickResult::Waiting;
        };
        let (meta, terminal) = match &pending.packet {
            PcmPacket::Chunk(chunk) => (Some(chunk.meta), chunk.meta.end_of_track.then_some(Ok(chunk.meta.segment))),
            PcmPacket::Failed { failure, .. } => (None, Some(Err(*failure))),
        };
        match self.port.forward.try_push(pending.packet) {
            Ok(()) => {
                if let Some(meta) = meta {
                    self.last_output = meta;
                    if meta.segment == self.source.cursor().segment {
                        if let Some(end) = pending.source_end {
                            self.source.commit_source_end(end);
                        }
                        if !meta.end_of_track && meta.frames > 0 {
                            self.source.admitted();
                        }
                        kithara::probe_event!(chunk_admitted, segment = meta.segment.get());
                    }
                }
                if let Some(terminal) = terminal {
                    self.terminal = Some(terminal);
                }
                self.port.signal();
                TickResult::Progress
            }
            Err(packet) => {
                self.pending = Some(PendingPacket {
                    packet,
                    source_end: pending.source_end,
                });
                TickResult::Backpressured
            }
        }
    }

    fn eof(&mut self) {
        let cursor = self.source.cursor();
        let source_span = (self.last_output.segment == cursor.segment)
            .then_some(self.last_output.source_span)
            .flatten()
            .and_then(|span| span.for_output_range(span.output_frames()..span.output_frames()));
        let position = source_span
            .and_then(|span| span.position_at(0))
            .unwrap_or_default();
        let mut samples = self.pools.get::<f32>();
        samples.clear();
        let meta = AudioChunkInfo {
            segment: cursor.segment,
            lane_frame: cursor.frame,
            timestamp: position,
            end_timestamp: position,
            frames: 0,
            end_of_track: true,
            source_span,
            ..self.last_output
        };
        self.pending = Some(PendingPacket {
            packet: PcmPacket::Chunk(AudioChunk::new(meta, samples)),
            source_end: None,
        });
    }
}

impl<T, S> LaneTask for DecoderNode<T, S>
where
    T: AudioSource<Chunk = AudioChunk>,
    S: HasPool<f32> + Send + Sync + 'static,
{
    fn set_priority(&mut self, class: ServiceClass) {
        self.priority = class;
    }

    fn poll_commands(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.source.poll_commands(cx)
    }
}

impl<T, S> Task for DecoderNode<T, S>
where
    T: AudioSource<Chunk = AudioChunk>,
    S: HasPool<f32> + Send + Sync + 'static,
{
    fn priority(&self) -> Option<Priority> {
        Some(self.priority.into())
    }

    fn on_cancel(&mut self) {
        if let Some(activity) = &mut self.activity {
            activity.set_playing(false);
        }
    }

    fn recycle(&mut self) {
        while let Some(packet) = self.port.reverse.try_pop() {
            self.retire(packet);
        }
        if let Some(activity) = &mut self.activity {
            activity.set_playing(*self.port.playing.read());
        }
        let _ = self.source.prepare_deferred();
        self.source.finish_deferred();
    }

    #[kithara::measure(label = "play.decoder.tick")]
    fn tick(&mut self) -> TickResult {
        if let Err(error) = self.synchronize() {
            self.fail(error);
        }
        if self.pending.is_some() {
            return self.admit();
        }
        if matches!(self.terminal, Some(Err(_))) || self.port.forward.is_full() {
            return TickResult::Backpressured;
        }
        let start = WallInstant::now();
        match self.source.step_track() {
            TrackStep::Produced(Fetch::Data { data, source_end }) => {
                self.terminal = None;
                if let Some(load) = &self.engine_load {
                    load.record(
                        start.elapsed(),
                        data.frames(),
                        data.spec().sample_rate.get(),
                    );
                }
                self.pending = Some(PendingPacket {
                    packet: PcmPacket::Chunk(data),
                    source_end,
                });
            }
            TrackStep::Produced(Fetch::NaturalEof) | TrackStep::Eof => {
                if self.terminal == Some(Ok(self.source.cursor().segment)) {
                    return TickResult::Backpressured;
                }
                self.eof();
            }
            TrackStep::Produced(Fetch::Failure { failure }) => {
                if self.terminal == Some(Ok(self.source.cursor().segment)) {
                    return TickResult::Backpressured;
                }
                self.fail(failure);
            }
            TrackStep::Failed(error) => {
                if self.terminal == Some(Ok(self.source.cursor().segment)) {
                    return TickResult::Backpressured;
                }
                self.fail(error);
            }
            TrackStep::StateChanged => {
                self.terminal = None;
                return TickResult::Progress;
            }
            TrackStep::Blocked(WaitingReason::WaitingDemand) => return TickResult::UpstreamPending,
            TrackStep::Blocked(WaitingReason::Waiting | WaitingReason::WaitingMetadata) => {
                return TickResult::Waiting;
            }
        }
        self.admit()
    }

    fn warm_up(&mut self) {
        self.source.warm_up();
    }
}

impl<T, S> Drop for DecoderNode<T, S> {
    fn drop(&mut self) {
        if let Some(activity) = &mut self.activity {
            activity.set_playing(false);
        }
    }
}

#[cfg(test)]
mod scheduler_tests {
    use kithara_audio::{
        AudioRead, AudioSource, ChunkOutcome, DecodeErrorKind, Fetch, PreloadGate,
        TrackFailureKind, TrackStep, WaitingReason,
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
        ServiceClass,
        test_pools::{Pools, pools, sample_buffer},
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
            TrackStep::Failed(TrackFailureKind::Decode {
                kind: DecodeErrorKind::InvalidData,
            })
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
            drop(trace);
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
            drop(trace);
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
            drop(trace);
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
                    break;
                }
                admitted_after(&trace, &handle, seen).await;
            }
            drop(trace);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_assets::AssetStore;
    use kithara_audio::{
        Audio, AudioConfig, AudioEvent, AudioRead, AudioReadError, AudioSource, ChunkOutcome,
        DecodeErrorKind, FailureSource, Fetch, NoResamplerBackend, PreloadGate, PreparedAudio,
        SeekOutcome, SourceEnd, TrackFailureKind, TrackStep, WaitingReason, mock::AudioSourceMock,
    };
    use kithara_command::{ChannelConfig, channel};
    use kithara_effects::EffectDrain;
    use kithara_events::{DeferredBus, EventBus};
    use kithara_platform::{
        CancelToken,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec};
    use kithara_stream::{
        PlayheadRead, PlayheadState, PlayheadWrite, SeekControl, SeekObserve, SeekState, Stream,
        mock::NoopWorkerWake,
    };
    use kithara_test_fixtures::{assets, unit_fixtures::eq_silence as node_silence};
    use kithara_test_utils::{cancel_token, kithara};
    use kithara_worker::{Task, TickResult};
    use ringbuf::traits::{Consumer, Split};
    use unimock::{MockFn, Unimock, matching};

    use super::*;
    use crate::{
        LaneProtocol, WarpSource,
        test_pools::{Pools, pools, sample_buffer},
        worker::EngineLoad,
    };

    type FileAudio = Audio<Stream<kithara_file::File<crate::test_pools::TestPools>>>;

    pub(super) async fn prepared_node<S>(
        source: S,
        capacity: usize,
        preload_chunks: usize,
    ) -> (DecoderNode<S>, FileAudio)
    where
        S: AudioSource<Chunk = AudioChunk>,
    {
        let prepared = prepared_file_audio(capacity, preload_chunks, None)
            .await
            .map(|audio, _| (audio, source));
        let (audio, lane) = prepared.into();
        (decoder_node(lane, Arc::new(SeekState::new())), audio)
    }

    async fn prepared_file_audio(
        capacity: usize,
        preload_chunks: usize,
        cancel: Option<CancelToken>,
    ) -> PreparedAudio<FileAudio, impl AudioSource<Chunk = AudioChunk>> {
        let pools = pools();
        let path = assets::signal_wav_sine440_120ms()
            .path()
            .expect("native WAV fixture path");
        let stream =
            kithara_file::FileConfig::for_src(kithara_file::FileSrc::Local(path.to_owned()))
                .store(AssetStore::builder(pools.clone()).build())
                .pools(pools.clone())
                .maybe_cancel(cancel.clone())
                .build();
        let config = AudioConfig::<_, NoResamplerBackend>::for_stream(stream)
            .audio_buffer_chunks(capacity)
            .preload_chunks(
                std::num::NonZeroUsize::new(preload_chunks).expect("non-zero preload threshold"),
            )
            .maybe_cancel(cancel)
            .build();
        Audio::prepare(config, Arc::new(NoopWorkerWake), pools)
            .await
            .unwrap_or_else(|error| panic!("prepare real audio lane: {error}"))
    }

    fn decoder_node<S>(
        lane: PreparedAudioLane<S>,
        seek_obs: Arc<dyn SeekObserve>,
    ) -> DecoderNode<S> {
        DecoderNode {
            source: lane.source,
            port: lane.port,
            seek_obs,
            preload_gate: lane.preload_gate,
            playhead: lane.playhead,
            emit: lane.emit,
            preload_chunks: lane.preload_chunks,
            engine_load: None,
            runtime: DecoderRuntime::default(),
        }
    }

    async fn real_file_node(
        cancel: CancelToken,
    ) -> (DecoderNode<impl AudioSource<Chunk = AudioChunk>>, FileAudio) {
        let prepared = prepared_file_audio(4, 1, Some(cancel)).await;
        let (audio, lane) = prepared.into();
        let seek_obs = lane.source.seek_observe();
        (decoder_node(lane, seek_obs), audio)
    }






    #[kithara::rtsan_forbid_blocking]
    fn checked_stream_terminal_reads(reader: &mut impl AudioRead) -> [bool; 3] {
        let mut samples = [0.0; 16];
        let interleaved = reader.read(&mut samples).is_err();
        let mut left = [0.0; 8];
        let mut right = [0.0; 8];
        let mut output = [&mut left[..], &mut right[..]];
        let planar = reader.read_planar(&mut output).is_err();
        let chunk = reader.next_chunk().is_err();
        [interleaved, planar, chunk]
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
