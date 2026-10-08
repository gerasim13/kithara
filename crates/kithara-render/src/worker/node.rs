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
        if self.terminal == Some(Ok(self.source.cursor().segment)) {
            return TickResult::Waiting;
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
                self.eof();
            }
            TrackStep::Produced(Fetch::Failure { failure }) => {
                self.fail(failure);
            }
            TrackStep::Failed(error) => {
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
        AudioSource, Fetch, TrackStep, WaitingReason, SeekOutcome,
    };
    use kithara_platform::{
        CancelToken,
        thread,
        time::{Duration, timeout as platform_timeout},
    };
    use kithara_signal::AudioChunk;
    use std::num::NonZeroU32;
    use kithara_command::Sender;
    use kithara_test_utils::kithara;
    use kithara_worker::{
        Dispatcher, DispatcherConfig, TaskConfig, TaskHandle, Worker, WorkerConfig,
    };

    use super::{tests::prepared_node, *};
    use crate::{
        ServiceClass,
        test_pools::{Pools, TestPools, pools},
        LaneProtocol, worker::PcmPacket,
    };

    fn empty_chunk(pools: &Pools, frame: u64) -> AudioChunk {
        let mut chunk = tests::empty_chunk(pools);
        chunk.meta.frame_offset = frame;
        chunk.meta.source_span = kithara_signal::SourceSpan::new(frame, frame + 1, chunk.spec().sample_rate, 1);
        chunk.meta.timestamp = chunk.spec().duration_for(frame).expect("timestamp");
        chunk.meta.end_timestamp = chunk.spec().duration_for(frame + 1).expect("end timestamp");
        chunk
    }

    struct MockSource {
        pools: Pools,
        ready: bool,
        should_panic: bool,
        chunks_to_produce: usize,
        cursor: usize,
    }

    impl MockSource {
        fn new(pools: Pools, chunks: usize) -> Self {
            Self {
                pools,
                chunks_to_produce: chunks,
                cursor: 0,
                ready: true,
                should_panic: false,
            }
        }
    }

    impl AudioSource for MockSource {
        type Chunk = AudioChunk;
        fn seek(&mut self, target: Duration) -> Result<SeekOutcome, kithara_audio::AudioReadError> {
            self.cursor = 0;
            Ok(SeekOutcome::Landed { target, landed_at: target })
        }
        fn set_host_sample_rate(&mut self, _rate: NonZeroU32) {}
        fn host_sample_rate(&self) -> Option<NonZeroU32> { None }

        fn step_track(&mut self) -> TrackStep<AudioChunk> {
            if !self.ready {
                return TrackStep::Blocked(WaitingReason::Waiting);
            }
            if self.should_panic {
                panic!("mock panic for testing");
            }
            if self.cursor >= self.chunks_to_produce {
                return TrackStep::Eof;
            }
            let frame = u64::try_from(self.cursor).expect("source cursor fits");
            self.cursor += 1;
            TrackStep::Produced(Fetch::data(empty_chunk(&self.pools, frame)))
        }
    }

    #[derive(Default)]
    struct FailingSource;

    impl AudioSource for FailingSource {
        type Chunk = AudioChunk;
        fn seek(&mut self, target: Duration) -> Result<SeekOutcome, kithara_audio::AudioReadError> {
            Ok(SeekOutcome::Landed { target, landed_at: target })
        }
        fn set_host_sample_rate(&mut self, _rate: NonZeroU32) {}
        fn host_sample_rate(&self) -> Option<NonZeroU32> { None }

        fn step_track(&mut self) -> TrackStep<AudioChunk> {
            TrackStep::Failed(TrackFailureKind::Decode { kind: kithara_audio::DecodeErrorKind::InvalidData })
        }
    }

    async fn make_node<S>(
        source: S,
        ringbuf_capacity: usize,
        preload_chunks: usize,
    ) -> (
        DecoderNode<S, TestPools>,
        impl FnMut() -> Option<()> + Send + 'static,
        Sender<LaneProtocol>,
    )
    where
        S: AudioSource<Chunk = AudioChunk>,
    {
        let (node, mut receiver, lane) = prepared_node(source, ringbuf_capacity, preload_chunks.max(1)).await;
        let pop = move || match receiver.pop() {
            Some(PcmPacket::Chunk(packet)) if !packet.meta.end_of_track => Some(()),
            _ => None,
        };
        (node, pop, lane)
    }

    struct PlaybackScheduler {
        dispatcher: Dispatcher,
        _worker: Worker,
    }

    impl PlaybackScheduler {
        fn register<S>(&self, node: DecoderNode<S, TestPools>) -> Result<TaskHandle, kithara_worker::TaskError>
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

    fn register<S>(handle: &PlaybackScheduler, node: DecoderNode<S, TestPools>) -> TaskHandle
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
    async fn worker_preload_gate_fires(#[case] chunks: usize, #[case] preload: usize, #[case] message: &str) {
        let (mut node, _receiver, _lane) = prepared_node(MockSource::new(pools(), chunks), 32, preload).await;
        platform_timeout(Duration::from_secs(1), node.preload()).await.expect(message).expect("preload succeeds");
        assert!(node.source.is_preloaded() || node.terminal.is_some());
    }

    #[kithara::test(tokio)]
    async fn worker_preload_reports_failure() {
        let (mut node, mut receiver, _lane) = prepared_node(FailingSource, 32, 8).await;
        assert!(platform_timeout(Duration::from_secs(1), node.preload()).await.expect("failure terminates preload").is_err());
        assert!(matches!(receiver.pop(), Some(PcmPacket::Failed { .. })));
    }

    #[kithara::test(tokio)]
    async fn worker_preload_gate_reopens_after_seek() {
        let (mut node, _receiver, mut lane) = prepared_node(MockSource::new(pools(), 10), 32, 1).await;
        platform_timeout(Duration::from_secs(1), node.preload()).await.expect("initial preload").expect("preload succeeds");
        assert!(node.source.is_preloaded());
        let id = tests::segment(&mut lane);
        node.synchronize().expect("new segment");
        assert!(!node.source.is_preloaded());
        platform_timeout(Duration::from_secs(1), node.preload()).await.expect("post-segment preload").expect("preload succeeds");
        assert!(node.source.is_preloaded());
        assert!(lane.receipts().any(|receipt| matches!(receipt.outcome(), kithara_command::Outcome::Applied { data, .. } if data.ready == Some(id))));
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
                /// Time each step holds the shared worker thread before producing.
            step: Duration,
            pools: Pools,
            cursor: u64,
        }

        impl AudioSource for EndlessSource {
            type Chunk = AudioChunk;
            fn seek(&mut self, target: Duration) -> Result<SeekOutcome, kithara_audio::AudioReadError> {
                self.cursor = 0;
                Ok(SeekOutcome::Landed { target, landed_at: target })
            }
            fn set_host_sample_rate(&mut self, _rate: NonZeroU32) {}
            fn host_sample_rate(&self) -> Option<NonZeroU32> { None }

            fn step_track(&mut self) -> TrackStep<AudioChunk> {
                thread::sleep(self.step);
                let chunk = empty_chunk(&self.pools, self.cursor);
                self.cursor += 1;
                TrackStep::Produced(Fetch::data(chunk))
            }
        }

        /// Source that never has data, so its node waits on every pass.
        struct WaitingSource {
                /// Time each step holds the shared worker thread before waiting.
            step: Duration,
        }

        impl AudioSource for WaitingSource {
            type Chunk = AudioChunk;
            fn seek(&mut self, target: Duration) -> Result<SeekOutcome, kithara_audio::AudioReadError> {
                Ok(SeekOutcome::Landed { target, landed_at: target })
            }
            fn set_host_sample_rate(&mut self, _rate: NonZeroU32) {}
            fn host_sample_rate(&self) -> Option<NonZeroU32> { None }

            fn step_track(&mut self) -> TrackStep<AudioChunk> {
                thread::sleep(self.step);
                TrackStep::Blocked(WaitingReason::Waiting)
            }
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
            let (node, mut pop, mut lane) = make_node(source, 32, 1).await;
            let _id = register(&handle, node);

            receive_chunks(&trace, &handle, &mut pop, 2).await;
            let seen = trace.events().len();
            pass_after(&trace, &handle, seen, |pass| {
                pass_field(pass, "backpressured") >= 1
            })
            .await;
            let seen = trace.events().len();
            let id = tests::segment(&mut lane);
            handle.wake_handle().wake();
            // The consumer must observe the seek and retire the full pre-seek ring.
            let _ = pop();
            trace
                .wait_for(|events| {
                    events[seen..]
                        .iter()
                        .any(|e| is(e, "chunk_admitted") && e.field("segment") == Some(id.get()))
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
                    cursor: 0,
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
    use std::num::{NonZeroU32, NonZeroUsize};
    use kithara_audio::{AudioSource, Fetch, SourceEnd, TrackStep, WaitingReason, SeekOutcome};
    use kithara_command::{Batch, ChannelConfig, Sender, When, channel};
    use kithara_effects::EffectDrain;
    use kithara_platform::{sync::{Arc, Mutex}, time::Duration};
    use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec, SegmentId};
    use kithara_test_fixtures::unit_fixtures::eq_silence as node_silence;
    use kithara_test_utils::kithara;
    use kithara_worker::{Task, TickResult};
    use super::*;
    use crate::{LaneCommand, LaneProtocol, WarpSource, test_pools::{Pools, TestPools, pools}, worker::{EngineLoad, PcmReceiver, packet_tests::{PacketRing, chunk}}};

    pub(super) async fn prepared_node<T>(source: T, capacity: usize, preload: usize) -> (DecoderNode<T, TestPools>, PcmReceiver, Sender<LaneProtocol>)
    where T: AudioSource<Chunk = AudioChunk> {
        let pools = pools();
        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("rate"));
        let (lane, inbox) = channel(ChannelConfig::builder().build());
        let config = kithara_warp::WarpConfig::builder()
            .source_block_frames(NonZeroUsize::new(65_536).expect("source block"))
            .build();
        let source = WarpSource::new(source, kithara_warp::Warp::new((), &config).renderer(spec, pools.clone()), Vec::new(), EffectDrain::new(0, &pools).expect("drain"), spec, pools.clone(), inbox, NonZeroUsize::new(preload).expect("preload"), crate::consts::DEFAULT_DECLICK);
        let (receiver, producer) = PacketRing::new(spec, Duration::from_secs(1), capacity).into_ends();
        (DecoderNode::new(source, producer, None, AudioChunkInfo { spec, ..AudioChunkInfo::default() }, None, pools), receiver, lane)
    }

    pub(super) fn empty_chunk(_pools: &Pools) -> AudioChunk {
        chunk(AudioSpec::new(2, NonZeroU32::new(44_100).expect("rate")), SegmentId::FIRST, 0, 0, &[0.0, 0.0])
    }

    pub(super) struct ScriptedSource {
        pub(super) steps: std::collections::VecDeque<TrackStep<AudioChunk>>,
        commits: Arc<Mutex<Vec<SourceEnd>>>,
    }

    impl ScriptedSource {
        pub(super) fn new(steps: impl IntoIterator<Item = TrackStep<AudioChunk>>) -> Self { Self { steps: steps.into_iter().collect(), commits: Arc::new(Mutex::new(Vec::new())) } }
    }

    impl AudioSource for ScriptedSource {
        type Chunk = AudioChunk;
        fn seek(&mut self, target: Duration) -> Result<SeekOutcome, kithara_audio::AudioReadError> { Ok(SeekOutcome::Landed { target, landed_at: target }) }
        fn set_host_sample_rate(&mut self, _rate: NonZeroU32) {}
        fn host_sample_rate(&self) -> Option<NonZeroU32> { None }
        fn commit_source_end(&mut self, end: SourceEnd) { self.commits.lock().push(end); }
        fn step_track(&mut self) -> TrackStep<AudioChunk> { self.steps.pop_front().unwrap_or(TrackStep::Eof) }
    }

    fn produced() -> TrackStep<AudioChunk> { TrackStep::Produced(Fetch::data(empty_chunk(&pools()))) }
    pub(super) fn segment(lane: &mut Sender<LaneProtocol>) -> SegmentId {
        let id = SegmentId::FIRST.next();
        lane.send(When::Next, Batch { basis: Vec::new(), commands: vec![LaneCommand::Segment { id, from: Duration::from_secs(1), speed: kithara_warp::SpeedCurve::Constant(1.0) }] }).expect("segment batch");
        id
    }

    #[kithara::test(tokio)]
    async fn worker_preload_gate_fires_on_failure() {
        let failure = TrackFailureKind::Decode { kind: kithara_audio::DecodeErrorKind::InvalidData };
        let (mut node, _receiver, _lane) = prepared_node(ScriptedSource::new([TrackStep::Failed(failure)]), 32, 8).await;
        let result = kithara_platform::time::timeout(Duration::from_secs(1), node.preload())
            .await.expect("decoder failure must complete the preload wait");
        assert_eq!(result, Err(failure));
        assert_eq!(node.terminal, Some(Err(failure)));
    }

    #[kithara::test(tokio)]
    async fn worker_telemetry_throttles_immediate_repeats() {
        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("rate"));
        let packet = chunk(spec, SegmentId::FIRST, 0, 0, &vec![0.0; 30_870]);
        let (mut node, mut receiver, _lane) = prepared_node(ScriptedSource::new([TrackStep::Produced(Fetch::data(packet))]), 4, 1).await;
        let meter = Arc::new(EngineLoad::default());
        meter.record(Duration::from_millis(5), 4_410, 44_100);
        node.engine_load = Some(Arc::clone(&meter));
        receiver.set_position(Duration::from_millis(100));
        assert_eq!(node.tick(), TickResult::Progress);
        let first = meter.snapshot();
        for _ in 0..2 {
            assert_eq!(receiver.cached_span(), Duration::from_millis(250));
            assert_eq!(receiver.decoded_frontier(), Duration::from_millis(350));
            assert!(meter.snapshot().is_active());
            assert_eq!(meter.snapshot().load(), first.load());
            assert_eq!(meter.snapshot().ms(), first.ms());
        }
        assert!(receiver.pop().is_some());
        assert!(receiver.pop().is_none(), "second immediate observation does not republish");
    }

    #[kithara::test(tokio)]
    async fn deferred_eof_event_keeps_the_decode_epoch() {
        let (mut node, mut receiver, mut lane) = prepared_node(ScriptedSource::new([]), 1, 1).await;
        assert_eq!(node.tick(), TickResult::Progress);
        assert_eq!(node.tick(), TickResult::Progress);
        let live_segment = segment(&mut lane);
        assert_eq!(live_segment, SegmentId::FIRST.next(), "seek overtakes the deferred EOF read");
        let mut eof_segments = std::iter::from_fn(|| receiver.pop()).filter_map(|packet| match packet {
            PcmPacket::Chunk(chunk) if chunk.meta.end_of_track => Some(chunk.meta.segment),
            _ => None,
        });
        assert_eq!(eof_segments.next(), Some(SegmentId::FIRST));
        assert_eq!(eof_segments.next(), None);
    }

    #[kithara::test(tokio)]
    #[case(WaitingReason::Waiting)]
    #[case(WaitingReason::WaitingDemand)]
    #[case(WaitingReason::WaitingMetadata)]
    async fn decoder_node_upstream_park_after_audio_opens_the_preload_gate(#[case] reason: WaitingReason) {
        let source = ScriptedSource::new([produced(), TrackStep::Blocked(reason)]);
        let (mut node, _receiver, _lane) = prepared_node(source, 4, 2).await;
        let _ = node.tick();
        assert!(!node.source.is_preloaded(), "the chunk quota is not met after one chunk");
        let _ = node.tick();
        assert!(node.source.is_preloaded(), "a producer parked on {reason:?} with audio behind it must not strand the construction wait");
    }

    #[kithara::test(tokio)]
    async fn decoder_node_eof_under_backpressure() {
        let (mut node, mut receiver, _lane) = prepared_node(ScriptedSource::new([produced()]), 1, 1).await;
        assert_eq!(node.tick(), TickResult::Progress);
        assert_eq!(node.tick(), TickResult::Backpressured);
        assert!(node.terminal.is_none());
        assert!(matches!(receiver.pop(), Some(PcmPacket::Chunk(packet)) if !packet.meta.end_of_track));
        assert_eq!(node.tick(), TickResult::Progress);
        assert_eq!(node.tick(), TickResult::Progress);
        assert!(node.terminal.is_some());
        assert!(matches!(receiver.pop(), Some(PcmPacket::Chunk(packet)) if packet.meta.end_of_track));
        assert_eq!(node.tick(), TickResult::Waiting);
        assert!(receiver.pop().is_none(), "current-segment EOF publishes exactly once");
    }

    #[kithara::test(tokio)]
    async fn decoder_node_does_not_republish_exhausted_warp_source_eof() {
        let (mut node, mut receiver, _lane) = prepared_node(ScriptedSource::new([]), 1, 1).await;
        assert_eq!(node.tick(), TickResult::Progress);
        assert_eq!(node.tick(), TickResult::Progress);
        assert!(matches!(receiver.pop(), Some(PcmPacket::Chunk(packet)) if packet.meta.end_of_track));
        assert_eq!(node.tick(), TickResult::Waiting);
        assert_eq!(node.tick(), TickResult::Waiting);
        assert!(receiver.pop().is_none(), "one terminal packet");
    }

    #[kithara::test(tokio)]
    async fn decoder_node_records_engine_load_on_produced(node_silence: Vec<f32>) {
        let meter = Arc::new(EngineLoad::default());
        assert!(!meter.snapshot().is_active(), "idle before any tick");
        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("rate"));
        let packet = chunk(spec, SegmentId::FIRST, 0, 0, &node_silence[..8_820]);
        let (mut node, _receiver, _lane) = prepared_node(ScriptedSource::new([TrackStep::Produced(Fetch::data(packet))]), 4, 1).await;
        node.engine_load = Some(Arc::clone(&meter));
        assert_eq!(node.tick(), TickResult::Progress);
        assert!(meter.snapshot().is_active(), "meter records Produced ticks");
    }

    #[kithara::test(tokio)]
    async fn worker_observation_reads_do_not_republish_packets() {
        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("rate"));
        let packet = chunk(spec, SegmentId::FIRST, 0, 0, &vec![0.0; 30_870]);
        let (mut node, mut receiver, _lane) = prepared_node(ScriptedSource::new([TrackStep::Produced(Fetch::data(packet))]), 4, 1).await;
        let meter = Arc::new(EngineLoad::default());
        meter.record(Duration::from_millis(5), 4_410, 44_100);
        node.engine_load = Some(Arc::clone(&meter));
        receiver.set_position(Duration::from_millis(100));
        assert_eq!(node.tick(), TickResult::Progress);
        for _ in 0..2 {
            assert_eq!(receiver.cached_span(), Duration::from_millis(250));
            assert_eq!(receiver.decoded_frontier(), Duration::from_millis(350));
            assert!(meter.snapshot().is_active());
        }
        assert!(receiver.pop().is_some());
        assert!(receiver.pop().is_none(), "observations do not duplicate output");
    }

    #[kithara::test(tokio)]
    async fn decoder_node_distinguishes_failed_from_eof_on_the_wire() {
        let (mut eof_node, mut eof_receiver, _lane) = prepared_node(ScriptedSource::new([]), 1, 1).await;
        assert_eq!(eof_node.tick(), TickResult::Progress);
        assert_eq!(eof_node.tick(), TickResult::Progress);
        let eof_marker = eof_receiver.pop();
        let failed = TrackStep::Failed(TrackFailureKind::Decode { kind: kithara_audio::DecodeErrorKind::InvalidData });
        let (mut failed_node, mut failed_receiver, _lane) = prepared_node(ScriptedSource::new([failed]), 1, 1).await;
        assert_eq!(failed_node.tick(), TickResult::Progress);
        let failed_marker = failed_receiver.pop();
        assert!(matches!(eof_marker, Some(PcmPacket::Chunk(packet)) if packet.meta.end_of_track));
        assert!(matches!(failed_marker, Some(PcmPacket::Failed { .. })));
    }

    #[kithara::test(tokio)]
    async fn an_admitted_eof_keeps_its_segment_after_a_later_segment_opens() {
        let (mut node, mut receiver, mut lane) = prepared_node(ScriptedSource::new([]), 1, 1).await;
        assert_eq!(node.tick(), TickResult::Progress);
        assert_eq!(node.tick(), TickResult::Progress);
        let next = segment(&mut lane);
        node.synchronize().expect("segment opens");
        assert_eq!(next, SegmentId::FIRST.next());
        assert_eq!(node.source.cursor().segment, next);
        let Some(PcmPacket::Chunk(eof)) = receiver.pop() else { panic!("EOF packet"); };
        assert!(eof.meta.end_of_track);
        assert_eq!(eof.meta.segment, SegmentId::FIRST);
        assert!(receiver.pop().is_none());
    }

    #[kithara::test(tokio)]
    async fn decoded_frontier_advances_only_after_final_port_admission() {
        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("rate"));
        let packet = chunk(spec, SegmentId::FIRST, 1, 1, &vec![0.0; 66_148]);
        let source = ScriptedSource::new([produced(), TrackStep::Produced(Fetch::data(packet))]);
        let (mut node, mut receiver, _lane) = prepared_node(source, 1, 1).await;
        let initial = spec.duration_for(1).expect("initial frame");
        assert_eq!(node.tick(), TickResult::Progress);
        assert_eq!(node.tick(), TickResult::Backpressured);
        assert_eq!(receiver.decoded_frontier(), initial);
        assert!(receiver.pop().is_some());
        assert_eq!(node.tick(), TickResult::Progress);
        assert_eq!(receiver.decoded_frontier(), Duration::from_millis(750));
    }

    #[kithara::test(tokio)]
    async fn source_end_commits_only_after_final_port_admission() {
        let end = SourceEnd::new(12_345, NonZeroU32::new(44_100).expect("rate"));
        let spec = AudioSpec::new(2, end.sample_rate());
        let packet = chunk(spec, SegmentId::FIRST, 0, 12_344, &[0.0; 2]);
        let source = ScriptedSource::new([TrackStep::Produced(Fetch::rendered(packet, end))]);
        let commits = Arc::clone(&source.commits);
        let (mut node, mut receiver, _lane) = prepared_node(source, 1, 1).await;
        node.pending = Some(PendingPacket {
            packet: PcmPacket::Chunk(empty_chunk(&pools())),
            source_end: None,
        });
        assert_eq!(node.tick(), TickResult::Progress);
        assert_eq!(node.tick(), TickResult::Backpressured);
        assert!(commits.lock().is_empty());
        assert!(receiver.pop().is_some());
        assert_eq!(node.tick(), TickResult::Progress);
        assert_eq!(commits.lock().as_slice(), &[end]);
    }

    #[kithara::test(tokio)]
    async fn decoder_node_live_upstream_demand_does_not_tick_hang_wait() {
        let source = ScriptedSource::new([TrackStep::Blocked(WaitingReason::WaitingDemand)]);
        let (mut node, _receiver, _lane) = prepared_node(source, 2, 1).await;
        assert_eq!(node.tick(), TickResult::UpstreamPending);
    }

    #[kithara::test(tokio)]
    #[case(WaitingReason::Waiting)]
    #[case(WaitingReason::WaitingDemand)]
    #[case(WaitingReason::WaitingMetadata)]
    async fn decoder_node_upstream_park_after_audio_waits_for_the_preload_quota(#[case] reason: WaitingReason) {
        let source = ScriptedSource::new([produced(), TrackStep::Blocked(reason)]);
        let (mut node, _receiver, _lane) = prepared_node(source, 4, 2).await;
        assert_eq!(node.tick(), TickResult::Progress);
        assert!(!node.source.is_preloaded(), "one chunk is below quota");
        let _ = node.tick();
        assert!(!node.source.is_preloaded(), "a park cannot replace the quota");
    }

    #[kithara::test(tokio)]
    #[case(WaitingReason::Waiting)]
    #[case(WaitingReason::WaitingDemand)]
    #[case(WaitingReason::WaitingMetadata)]
    async fn decoder_node_park_without_audio_keeps_the_preload_gate_shut(#[case] reason: WaitingReason) {
        let (mut node, _receiver, _lane) = prepared_node(ScriptedSource::new([TrackStep::Blocked(reason)]), 2, 1).await;
        let _ = node.tick();
        assert!(!node.source.is_preloaded(), "a park with nothing emitted is not preload");
    }

    #[kithara::test(tokio)]
    async fn decoder_node_preload_gate_stays_shut_below_the_chunk_quota() {
        let (mut node, _receiver, _lane) = prepared_node(ScriptedSource::new([produced()]), 4, 2).await;
        assert_eq!(node.tick(), TickResult::Progress);
        assert!(!node.source.is_preloaded(), "one chunk of a two-chunk quota is not preload");
    }

    #[kithara::test(tokio)]
    async fn decoder_node_seek_rearms_preload_gate() {
        let source = ScriptedSource::new([produced(), TrackStep::StateChanged, produced()]);
        let (mut node, mut receiver, mut lane) = prepared_node(source, 1, 1).await;
        assert_eq!(node.tick(), TickResult::Progress);
        assert!(node.source.is_preloaded(), "first chunk opens preload");
        let id = segment(&mut lane);
        assert_eq!(node.tick(), TickResult::Backpressured);
        assert!(!node.source.is_preloaded(), "new segment rearms preload");
        assert_eq!(node.source.cursor().segment, id);
        assert!(matches!(receiver.pop(), Some(PcmPacket::Chunk(packet)) if packet.meta.segment == SegmentId::FIRST));
        assert_eq!(node.tick(), TickResult::Progress);
        assert!(!node.source.is_preloaded(), "StateChanged is not admitted PCM");
        assert_eq!(node.tick(), TickResult::Progress);
        assert!(node.source.is_preloaded(), "new-segment refill opens preload");
        assert!(matches!(receiver.pop(), Some(PcmPacket::Chunk(packet)) if packet.meta.segment == id));
        assert!(lane.receipts().any(|receipt| matches!(receipt.outcome(), kithara_command::Outcome::Applied { data, .. } if data.ready == Some(id))));
    }
}
