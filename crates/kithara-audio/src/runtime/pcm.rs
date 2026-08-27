use std::{
    num::NonZeroUsize,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

use kithara_platform::CancelToken;

use super::{Node, Scheduler, SchedulerHandle};
use crate::renderer::{DecoderNode, HangWatchdogObserver, TrackRegistration};

type TrackId = u64;

/// Monotonic counter for generating unique [`TrackId`] values.
struct TrackIdGen(AtomicU64);

impl TrackIdGen {
    // ast-grep-ignore: style.prefer-default-derive
    const fn new() -> Self {
        Self(AtomicU64::new(1))
    }

    fn next(&self) -> TrackId {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

/// Opaque PCM producer task prepared by `kithara-audio` and scheduled by
/// `kithara-play`.
#[doc(hidden)]
pub struct PcmTask(Box<dyn Node>);

impl From<TrackRegistration> for PcmTask {
    fn from(registration: TrackRegistration) -> Self {
        Self(Box::new(DecoderNode::from(registration)))
    }
}

/// Identifier of one registered PCM task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[doc(hidden)]
pub struct PcmTaskId(TrackId);

/// Registration failure reported by the low-level PCM scheduler.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[doc(hidden)]
#[non_exhaustive]
pub enum PcmSchedulerError {
    /// The configured task capacity has been reached.
    #[error("PCM scheduler capacity {capacity} reached")]
    Capacity { capacity: usize },
    /// The scheduler loop has already stopped.
    #[error("PCM scheduler stopped")]
    Stopped,
}

/// Wait-free wake capability retained by the prepared audio reader.
///
/// It can only request a scheduler pass; it cannot register, unregister or
/// shut down the worker.
#[derive(Clone)]
#[doc(hidden)]
pub struct PcmWake {
    inner: SchedulerHandle<Box<dyn Node>>,
}

impl PcmWake {
    delegate::delegate! {
        to self.inner {
            /// Wake immediately from an off-real-time thread.
            pub fn wake(&self);
            /// Coalesce a future pass without a syscall from the real-time path.
            #[call(defer_wake)]
            pub fn defer(&self);
        }
    }
}

impl kithara_stream::WorkerWake for PcmWake {
    fn wake(&self) {
        Self::wake(self);
    }
}

/// Transitional concrete port over the existing generic scheduler kernel.
///
/// `kithara-play::PlayWorker` is the sole owner and public construction path.
/// This port exists only to keep `kithara-audio` independent of `kithara-play`
/// while the analyzer still shares the generic scheduler implementation.
#[doc(hidden)]
pub struct PcmScheduler {
    active: AtomicUsize,
    capacity: NonZeroUsize,
    id_gen: TrackIdGen,
    inner: SchedulerHandle<Box<dyn Node>>,
}

impl PcmScheduler {
    /// Register one opaque task without exposing the generic runtime.
    ///
    /// # Errors
    ///
    /// Returns [`PcmSchedulerError::Capacity`] when the configured task limit
    /// has been reached, or [`PcmSchedulerError::Stopped`] after the scheduler
    /// loop has stopped.
    pub fn register(&self, task: PcmTask) -> Result<PcmTaskId, PcmSchedulerError> {
        self.active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < self.capacity.get()).then_some(active + 1)
            })
            .map_err(|_| PcmSchedulerError::Capacity {
                capacity: self.capacity.get(),
            })?;

        let id = self.id_gen.next();
        if !self.inner.register(id, task.0) {
            self.active.fetch_sub(1, Ordering::AcqRel);
            return Err(PcmSchedulerError::Stopped);
        }
        Ok(PcmTaskId(id))
    }

    /// Start the existing scheduler loop for a play-owned worker.
    #[must_use]
    pub fn start(name: String, cancel: CancelToken, capacity: NonZeroUsize) -> Self {
        let id_gen = TrackIdGen::new();
        let inner = Scheduler::<Box<dyn Node>, HangWatchdogObserver>::start(
            name,
            HangWatchdogObserver::new(),
            cancel,
        );

        Self {
            active: AtomicUsize::new(0),
            capacity,
            id_gen,
            inner,
        }
    }

    /// Remove one task without affecting any sibling registration.
    pub fn unregister(&self, task_id: PcmTaskId) {
        self.inner.unregister(task_id.0);
        let decremented = self
            .active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                active.checked_sub(1)
            });
        debug_assert!(decremented.is_ok(), "PCM scheduler registration underflow");
    }

    /// Create the restricted wake capability passed into audio preparation.
    #[must_use]
    pub fn wake_handle(&self) -> PcmWake {
        PcmWake {
            inner: self.inner.clone(),
        }
    }

    fn shutdown(&self) {
        self.inner.shutdown();
    }
}

impl Drop for PcmScheduler {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use kithara_bufpool::PcmPool;
    use kithara_decode::{PcmChunk, PcmMeta};
    use kithara_events::{DeferredBus, Event, EventBus};
    use kithara_platform::{
        sync::Arc,
        thread::sleep as thread_sleep,
        time::{Duration, Instant, timeout as platform_timeout},
    };
    use kithara_stream::{PlayheadRead, PlayheadState, SeekObserve, SeekState};
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        pipeline::{
            fetch::Fetch,
            track::{TrackStep, WaitingReason},
        },
        renderer::{MockSource, PreloadGate, ServiceClass, ThreadWake},
        runtime::{AtomicServiceClass, connect},
        traits::PcmSource,
    };

    fn empty_chunk() -> PcmChunk {
        PcmChunk::new(PcmMeta::default(), PcmPool::default().attach(Vec::new()))
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

    impl PcmSource for FailingSource {
        type Chunk = PcmChunk;

        fn seek_observe(&self) -> Arc<dyn SeekObserve> {
            Arc::clone(&self.seek_obs)
        }

        fn step_track(&mut self) -> TrackStep<PcmChunk> {
            TrackStep::Failed
        }
    }

    fn make_registration<S>(
        source: S,
        ringbuf_capacity: usize,
        preload_chunks: usize,
    ) -> (
        TrackRegistration,
        crate::runtime::Inlet<Fetch<PcmChunk>>,
        Arc<PreloadGate>,
    )
    where
        S: PcmSource<Chunk = PcmChunk> + 'static,
    {
        let wake = Arc::new(ThreadWake::default());
        let (outlet, inlet) = connect::<Fetch<PcmChunk>>(ringbuf_capacity, Some(wake));
        let (_trash_outlet, trash_inlet) = connect::<PcmChunk>(ringbuf_capacity + 2, None);
        let preload_gate = Arc::new(PreloadGate::default());

        let reg = TrackRegistration {
            outlet,
            trash_inlet,
            preload_chunks,
            source: Box::new(source),
            preload_gate: Arc::clone(&preload_gate),
            playhead: Arc::new(PlayheadState::new()) as Arc<dyn PlayheadRead>,
            emit: Arc::new(DeferredBus::<Event>::new(EventBus::new(8), 8)),
            service_class: Arc::new(AtomicServiceClass::new(ServiceClass::Audible)),
            engine_load: None,
        };
        (reg, inlet, preload_gate)
    }

    fn wait_for_chunks(
        rx: &mut crate::runtime::Inlet<Fetch<PcmChunk>>,
        count: usize,
        timeout: Duration,
    ) -> usize {
        let start = Instant::now();
        let mut received = 0;
        while received < count && start.elapsed() < timeout {
            if rx.try_pop().is_some() {
                received += 1;
            } else {
                thread_sleep(Duration::from_millis(1));
            }
        }
        received
    }

    fn test_scheduler() -> PcmScheduler {
        PcmScheduler::start(
            "kithara-play-worker-test".into(),
            CancelToken::never(),
            NonZeroUsize::new(8).expect("test capacity is non-zero"),
        )
    }

    fn scheduler_with_capacity(capacity: usize) -> PcmScheduler {
        PcmScheduler::start(
            "kithara-play-worker-capacity-test".into(),
            CancelToken::never(),
            NonZeroUsize::new(capacity).expect("test capacity is non-zero"),
        )
    }

    fn register(handle: &PcmScheduler, registration: TrackRegistration) -> PcmTaskId {
        handle
            .register(PcmTask::from(registration))
            .expect("test PCM task must register")
    }

    #[kithara::test]
    fn track_id_gen_produces_unique_ids() {
        let id_gen = TrackIdGen::new();
        let a = id_gen.next();
        let b = id_gen.next();
        let c = id_gen.next();
        assert_eq!(a, 1);
        assert_eq!(b, 2);
        assert_eq!(c, 3);
    }

    #[kithara::test]
    fn worker_creates_and_drops_cleanly() {
        let handle = test_scheduler();
        thread_sleep(Duration::from_millis(10));
        handle.shutdown();
        thread_sleep(Duration::from_millis(50));
    }

    #[kithara::test]
    fn registration_reports_capacity_exhaustion() {
        let handle = scheduler_with_capacity(1);
        let (first, _, _) = make_registration(MockSource::new(1), 2, 1);
        let (second, _, _) = make_registration(MockSource::new(1), 2, 1);

        let first_id = register(&handle, first);
        let result = handle.register(PcmTask::from(second));

        assert_eq!(result, Err(PcmSchedulerError::Capacity { capacity: 1 }));
        handle.unregister(first_id);
    }

    #[kithara::test]
    fn registration_reports_stopped_scheduler() {
        let handle = test_scheduler();
        handle.shutdown();
        let (registration, _, _) = make_registration(MockSource::new(1), 2, 1);

        let result = handle.register(PcmTask::from(registration));

        assert_eq!(result, Err(PcmSchedulerError::Stopped));
    }

    #[kithara::test]
    fn worker_delivers_chunks() {
        let handle = test_scheduler();
        let (reg, mut data_rx, _preload_gate) = make_registration(MockSource::new(10), 32, 3);

        let _id = register(&handle, reg);

        let received = wait_for_chunks(&mut data_rx, 5, Duration::from_secs(5));
        assert!(received >= 5, "expected >=5 chunks, got {received}");

        handle.shutdown();
    }

    #[kithara::test]
    fn worker_multi_track_round_robin() {
        let handle = test_scheduler();

        let (reg_a, mut rx_a, _) = make_registration(MockSource::new(10), 32, 1);
        let (reg_b, mut rx_b, _) = make_registration(MockSource::new(10), 32, 1);

        let _id_a = register(&handle, reg_a);
        let _id_b = register(&handle, reg_b);

        let a = wait_for_chunks(&mut rx_a, 3, Duration::from_secs(5));
        let b = wait_for_chunks(&mut rx_b, 3, Duration::from_secs(5));
        assert!(a >= 3, "track A: expected >=3 chunks, got {a}");
        assert!(b >= 3, "track B: expected >=3 chunks, got {b}");

        handle.shutdown();
    }

    #[kithara::test]
    fn worker_skips_not_ready_tracks() {
        let handle = test_scheduler();

        let (reg_a, mut rx_a, _) = make_registration(MockSource::new(10), 32, 1);
        let (reg_b, mut rx_b, _) = make_registration(MockSource::not_ready(10), 32, 1);

        let _id_a = register(&handle, reg_a);
        let _id_b = register(&handle, reg_b);

        thread_sleep(Duration::from_millis(100));

        let a = wait_for_chunks(&mut rx_a, 1, Duration::from_millis(100));
        let b = wait_for_chunks(&mut rx_b, 1, Duration::from_millis(50));
        assert!(a >= 1, "track A should receive chunks");
        assert_eq!(b, 0, "track B should receive nothing (not ready)");

        handle.shutdown();
    }

    #[kithara::test]
    fn worker_overflow_on_full_ringbuf() {
        let handle = test_scheduler();

        let (reg, mut rx, _) = make_registration(MockSource::new(5), 1, 1);

        let _id = register(&handle, reg);

        thread_sleep(Duration::from_millis(50));

        let first = rx.try_pop();
        assert!(first.is_some(), "should have at least one chunk");

        thread_sleep(Duration::from_millis(50));

        let second = rx.try_pop();
        assert!(second.is_some(), "overflow slot should have been flushed");

        handle.shutdown();
    }

    #[kithara::test]
    fn worker_panic_isolation() {
        let handle = test_scheduler();

        let (reg_a, _, _) = make_registration(MockSource::panicking(), 32, 1);
        let (reg_b, mut rx_b, _) = make_registration(MockSource::new(10), 32, 1);

        let _id_a = register(&handle, reg_a);
        let _id_b = register(&handle, reg_b);

        let b = wait_for_chunks(&mut rx_b, 3, Duration::from_secs(5));
        assert!(
            b >= 3,
            "track B should keep working after track A panics, got {b}"
        );

        handle.shutdown();
    }

    #[kithara::test]
    fn worker_seek_enters_pending_reset() {
        let handle = test_scheduler();

        let source = MockSource::new(100);
        let seek = Arc::clone(&source.seek);
        let (reg, mut rx, _) = make_registration(source, 32, 1);

        let _id = register(&handle, reg);

        let got = wait_for_chunks(&mut rx, 2, Duration::from_secs(5));
        assert!(got >= 2);

        let _ = seek.begin(Duration::from_secs(10));
        handle.wake_handle().wake();

        thread_sleep(Duration::from_millis(100));

        let after_seek = wait_for_chunks(&mut rx, 1, Duration::from_secs(5));
        assert!(after_seek >= 1, "should resume decoding after seek");

        handle.shutdown();
    }

    #[kithara::test(tokio)]
    async fn worker_preload_gate_fires_on_progress() {
        let handle = test_scheduler();

        let (reg, _rx, preload_gate) = make_registration(MockSource::new(10), 32, 3);

        let _id = register(&handle, reg);

        platform_timeout(Duration::from_secs(1), preload_gate.wait())
            .await
            .expect("preload gate must open once the preload threshold is met");
        assert!(preload_gate.is_ready());

        handle.shutdown();
    }

    #[kithara::test(tokio)]
    async fn worker_preload_gate_fires_on_eof() {
        let handle = test_scheduler();

        let (reg, _rx, preload_gate) = make_registration(MockSource::new(0), 32, 8);

        let _id = register(&handle, reg);

        platform_timeout(Duration::from_secs(1), preload_gate.wait())
            .await
            .expect("EOF before the preload threshold must still open the gate");
        assert!(preload_gate.is_ready());

        handle.shutdown();
    }

    #[kithara::test(tokio)]
    async fn worker_preload_gate_fires_on_failure() {
        let handle = test_scheduler();

        let (reg, _rx, preload_gate) = make_registration(FailingSource::default(), 32, 8);

        let _id = register(&handle, reg);

        platform_timeout(Duration::from_secs(1), preload_gate.wait())
            .await
            .expect("a decoder failure must open the gate so preload never stalls");
        assert!(preload_gate.is_ready());

        handle.shutdown();
    }

    #[kithara::test(tokio)]
    async fn worker_preload_gate_reopens_after_seek() {
        let handle = test_scheduler();

        let source = MockSource::new(10);
        let seek = Arc::clone(&source.seek);
        let (reg, _rx, preload_gate) = make_registration(source, 32, 1);
        let _id = register(&handle, reg);

        platform_timeout(Duration::from_secs(1), preload_gate.wait())
            .await
            .expect("initial preload gate must open");
        assert!(preload_gate.is_ready());

        let epoch = seek.begin(Duration::from_secs(1));
        handle.wake_handle().wake();

        platform_timeout(Duration::from_secs(1), preload_gate.wait_for_epoch(epoch))
            .await
            .expect("re-armed preload gate must reopen after the seek refills");

        handle.shutdown();
    }

    #[kithara::test]
    fn worker_unregister_removes_track() {
        let handle = test_scheduler();

        let (reg, mut rx, _) = make_registration(MockSource::new(100), 32, 1);

        let id = register(&handle, reg);

        let got = wait_for_chunks(&mut rx, 2, Duration::from_secs(5));
        assert!(got >= 2);

        handle.unregister(id);
        thread_sleep(Duration::from_millis(50));

        while rx.try_pop().is_some() {}

        thread_sleep(Duration::from_millis(50));
        assert!(rx.try_pop().is_none(), "no chunks after unregister");

        handle.shutdown();
    }

    #[kithara::test]
    fn unregister_one_task_keeps_sibling_running_and_releases_capacity() {
        let handle = scheduler_with_capacity(2);
        let (reg_a, mut rx_a, _) = make_registration(MockSource::new(100), 1, 1);
        let (reg_b, mut rx_b, _) = make_registration(MockSource::new(100), 1, 1);

        let id_a = register(&handle, reg_a);
        let id_b = register(&handle, reg_b);
        assert_eq!(wait_for_chunks(&mut rx_a, 1, Duration::from_secs(1)), 1);
        assert_eq!(wait_for_chunks(&mut rx_b, 1, Duration::from_secs(1)), 1);

        handle.unregister(id_a);

        let (reg_c, _rx_c, _) = make_registration(MockSource::new(1), 1, 1);
        let id_c = handle
            .register(PcmTask::from(reg_c))
            .expect("unregister must release one capacity slot");

        while rx_b.try_pop().is_some() {}
        handle.wake_handle().wake();
        assert_eq!(
            wait_for_chunks(&mut rx_b, 1, Duration::from_secs(1)),
            1,
            "unregistering one task must not stop its sibling"
        );

        handle.unregister(id_b);
        handle.unregister(id_c);
        handle.shutdown();
    }

    // Audible-before-Idle is a pure ordering property of the scheduler's slot
    // sequence, not of consumed-chunk counts; verifying it through a live ring
    // drain is structurally racy. It is locked deterministically in
    // `runtime::scheduler::tests` instead -- see
    // `refresh_reorders_live_when_atomic_service_class_changes`.

    /// A slow/blocked track must not starve a producing track.
    ///
    /// Reproduces the production bug: HLS track waiting for network data
    /// blocks the shared worker's `step_track()` call, causing MP3 track
    /// audio to stutter.
    ///
    /// The mock simulates a track whose `step_track()` blocks the thread
    /// for 50ms (like a real `wait_range()` call waiting for network data).
    /// The worker must still deliver chunks to the ready track at a
    /// rate sufficient for glitch-free playback.
    #[kithara::test]
    fn shared_worker_blocking_track_does_not_starve_producing_track() {
        struct BlockingSource {
            seek_obs: Arc<dyn SeekObserve>,
            blocking: Arc<AtomicBool>,
        }

        impl PcmSource for BlockingSource {
            type Chunk = PcmChunk;

            fn step_track(&mut self) -> TrackStep<PcmChunk> {
                if self.blocking.load(Ordering::Relaxed) {
                    thread_sleep(Duration::from_millis(10));
                    TrackStep::Blocked(WaitingReason::Waiting)
                } else {
                    TrackStep::Blocked(WaitingReason::Waiting)
                }
            }

            fn seek_observe(&self) -> Arc<dyn SeekObserve> {
                Arc::clone(&self.seek_obs)
            }
        }

        let handle = test_scheduler();

        let (reg_a, mut rx_a, _) = make_registration(MockSource::new(100), 32, 0);
        let _id_a = register(&handle, reg_a);

        let blocking = Arc::new(AtomicBool::new(true));
        let blocking_source = BlockingSource {
            seek_obs: Arc::new(SeekState::new()) as Arc<dyn SeekObserve>,
            blocking: Arc::clone(&blocking),
        };
        let (reg_b, _rx_b, _) = make_registration(blocking_source, 32, 0);
        let _id_b = register(&handle, reg_b);

        thread_sleep(Duration::from_millis(500));

        let mut got_a = 0;
        while rx_a.try_pop().is_some() {
            got_a += 1;
        }

        assert!(
            got_a >= 11,
            "Producing track must not be starved by blocking track: \
             got only {got_a} chunks in 1s (expected >=11 for glitch-free)"
        );

        blocking.store(false, Ordering::Relaxed);
        handle.shutdown();
    }

    /// A track whose `step_track()` blocks must not set the delivery rate of
    /// every other track on the shared worker.
    ///
    /// The worker runs one pass over all slots and calls `tick()` -- hence
    /// `step_track()` -- exactly once per slot per pass, so a slot that blocks
    /// for `BLOCK_MS` caps the pass rate at `1000 / BLOCK_MS` per second. With
    /// one chunk produced per tick, every other track is capped at that same
    /// rate no matter how fast its own source is. In production the blocking
    /// step is a decode that holds the `SharedStream` mutex, or a CPU-bound
    /// demuxer step (see `apple::audio_file_demuxer`, 10-77 ms).
    ///
    /// The oracle counts chunks, not wall-clock gaps: a ready track must be
    /// able to drain its whole source within a bounded number of polls. A gap
    /// oracle cannot see this defect -- at `BLOCK_MS` the gap stays near
    /// `BLOCK_MS`, well inside any limit chosen for glitch-free playback,
    /// while the track still delivers an order of magnitude too few chunks.
    #[kithara::test]
    fn shared_worker_sync_blocking_step_starves_other_tracks() {
        /// Chunks the fast track's source can produce before EOF.
        const SOURCE_CHUNKS: u32 = 1000;
        /// Polls the consumer is allowed before the source must be drained.
        const POLL_BUDGET: u32 = 600;
        /// How long the slow track holds the worker inside one step.
        const BLOCK_MS: u64 = 10;
        struct SlowDecodeSource {
            seek_obs: Arc<dyn SeekObserve>,
            block_ms: u64,
        }

        impl PcmSource for SlowDecodeSource {
            type Chunk = PcmChunk;

            fn step_track(&mut self) -> TrackStep<PcmChunk> {
                thread_sleep(Duration::from_millis(self.block_ms));
                TrackStep::Produced(Fetch::data(empty_chunk(), 0))
            }

            fn seek_observe(&self) -> Arc<dyn SeekObserve> {
                Arc::clone(&self.seek_obs)
            }
        }

        let handle = test_scheduler();

        let (reg_a, mut rx_a, _) =
            make_registration(MockSource::new(SOURCE_CHUNKS as usize), 32, 0);
        let _id_a = register(&handle, reg_a);

        let slow_source = SlowDecodeSource {
            seek_obs: Arc::new(SeekState::new()) as Arc<dyn SeekObserve>,
            block_ms: BLOCK_MS,
        };
        let (reg_b, mut rx_b, _) = make_registration(slow_source, 32, 0);
        let _id_b = register(&handle, reg_b);

        let mut delivered = 0u32;
        let mut polls = 0u32;

        let mut deepest_poll = 0u32;

        while delivered < SOURCE_CHUNKS && polls < POLL_BUDGET {
            let mut this_poll = 0u32;
            while rx_a.try_pop().is_some() {
                delivered += 1;
                this_poll += 1;
            }
            deepest_poll = deepest_poll.max(this_poll);
            while rx_b.try_pop().is_some() {}
            polls += 1;
            // The budget has to bound the pipeline, not the host. A real pause
            // makes these polls a window of real seconds, and how many chunks
            // arrive inside it is a property of the machine -- which is how this
            // same budget passed on an idle host and failed on a loaded one.
            // `park_timeout` is on the macro's rewrite list, so the window is
            // virtual and advances only once the worker has parked.
            thread::park_timeout(Duration::from_millis(5));
        }

        handle.shutdown();

        // `delivered` may overshoot by the EOF marker, which `try_pop` hands
        // over like any other item once the source is exhausted.
        assert!(
            delivered >= SOURCE_CHUNKS,
            "fast track drained {delivered} of {SOURCE_CHUNKS} chunks in {polls} polls \
             (deepest single poll {deepest_poll}) while a co-scheduled track blocked \
             {BLOCK_MS}ms per step -- the blocking step sets the worker's pass rate and \
             every other track is capped at one chunk per pass"
        );
    }
}
