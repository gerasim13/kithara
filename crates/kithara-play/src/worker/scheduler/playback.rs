use std::{
    num::NonZeroUsize,
    sync::atomic::{AtomicU64, Ordering},
};

use kithara_platform::CancelToken;
use portable_atomic::AtomicUsize;

use super::{Node, PlaybackObserver, Scheduler, SchedulerHandle};

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

/// Opaque playback task erased by `kithara-play` at registration.
#[doc(hidden)]
pub(crate) struct Task(Box<dyn Node>);

impl Task {
    /// Erase one concrete worker node at the scheduler registration boundary.
    #[must_use]
    pub(crate) fn new<N>(node: N) -> Self
    where
        N: Node,
    {
        Self(Box::new(node))
    }
}

/// Identifier of one registered playback task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[doc(hidden)]
pub(crate) struct TaskId(TrackId);

/// Registration failure reported by the playback scheduler.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[doc(hidden)]
#[non_exhaustive]
pub(crate) enum SchedulerError {
    /// The configured task capacity has been reached.
    #[error("playback scheduler capacity {capacity} reached")]
    Capacity { capacity: usize },
    /// The scheduler loop has already stopped.
    #[error("playback scheduler stopped")]
    Stopped,
}

/// Wait-free wake capability retained by the prepared audio reader.
///
/// It can only request a scheduler pass; it cannot register, unregister or
/// shut down the worker.
#[derive(Clone)]
#[doc(hidden)]
pub(crate) struct Wake {
    inner: SchedulerHandle<Box<dyn Node>>,
}

impl Wake {
    delegate::delegate! {
        to self.inner {
            /// Wake immediately from an off-real-time thread.
            pub(crate) fn wake(&self);
            /// Coalesce a future pass without a syscall from the real-time path.
            #[call(defer_wake)]
            pub(crate) fn defer(&self);
        }
    }
}

impl kithara_stream::WorkerWake for Wake {
    fn wake(&self) {
        Self::wake(self);
    }

    fn defer(&self) {
        Self::defer(self);
    }
}

/// Private scheduler state owned exclusively by [`super::super::PlayWorker`].
pub(crate) struct PlaybackScheduler {
    active: AtomicUsize,
    capacity: NonZeroUsize,
    id_gen: TrackIdGen,
    inner: SchedulerHandle<Box<dyn Node>>,
}

impl PlaybackScheduler {
    /// Register one opaque task without exposing the generic runtime.
    ///
    /// # Errors
    ///
    /// Returns [`SchedulerError::Capacity`] when the configured task limit
    /// has been reached, or [`SchedulerError::Stopped`] after the scheduler
    /// loop has stopped.
    pub(crate) fn register(&self, task: Task) -> Result<TaskId, SchedulerError> {
        self.active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < self.capacity.get()).then_some(active + 1)
            })
            .map_err(|_| SchedulerError::Capacity {
                capacity: self.capacity.get(),
            })?;

        let id = self.id_gen.next();
        if !self.inner.register(id, task.0) {
            self.active.fetch_sub(1, Ordering::AcqRel);
            return Err(SchedulerError::Stopped);
        }
        Ok(TaskId(id))
    }

    /// Start the existing scheduler loop for a play-owned worker.
    #[must_use]
    pub(crate) fn start(name: String, cancel: CancelToken, capacity: NonZeroUsize) -> Self {
        let id_gen = TrackIdGen::new();
        let inner = Scheduler::<Box<dyn Node>, PlaybackObserver>::start(
            name,
            PlaybackObserver::new(),
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
    pub(crate) fn unregister(&self, task_id: TaskId) {
        self.inner.unregister(task_id.0);
        let decremented = self
            .active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                active.checked_sub(1)
            });
        debug_assert!(
            decremented.is_ok(),
            "playback scheduler registration underflow"
        );
    }

    /// Create the restricted wake capability passed into audio preparation.
    #[must_use]
    pub(crate) fn wake_handle(&self) -> Wake {
        Wake {
            inner: self.inner.clone(),
        }
    }

    fn shutdown(&self) {
        self.inner.shutdown();
    }
}

impl Drop for PlaybackScheduler {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{super::TickResult, *};

    struct ParkedNode;

    impl Node for ParkedNode {
        fn tick(&mut self) -> TickResult {
            TickResult::Backpressured
        }
    }

    fn scheduler(capacity: usize) -> PlaybackScheduler {
        PlaybackScheduler::start(
            "kithara-playback-scheduler-test".into(),
            CancelToken::never(),
            NonZeroUsize::new(capacity).expect("test capacity is non-zero"),
        )
    }

    #[kithara::test]
    fn track_id_gen_produces_unique_ids() {
        let id_gen = TrackIdGen::new();
        assert_eq!(id_gen.next(), 1);
        assert_eq!(id_gen.next(), 2);
        assert_eq!(id_gen.next(), 3);
    }

    #[kithara::test]
    fn scheduler_creates_and_stops_cleanly() {
        let scheduler = scheduler(8);
        scheduler.shutdown();
    }

    #[kithara::test]
    fn registration_reports_capacity_exhaustion() {
        let scheduler = scheduler(1);
        let id = scheduler
            .register(Task::new(ParkedNode))
            .expect("first task must register");
        let result = scheduler.register(Task::new(ParkedNode));

        assert_eq!(result, Err(SchedulerError::Capacity { capacity: 1 }));
        scheduler.unregister(id);
    }

    #[kithara::test]
    fn registration_reports_stopped_scheduler() {
        let scheduler = scheduler(1);
        scheduler.shutdown();

        let result = scheduler.register(Task::new(ParkedNode));

        assert_eq!(result, Err(SchedulerError::Stopped));
    }

    #[kithara::test]
    fn unregister_releases_capacity() {
        let scheduler = scheduler(1);
        let first = scheduler
            .register(Task::new(ParkedNode))
            .expect("first task must register");
        scheduler.unregister(first);

        let second = scheduler
            .register(Task::new(ParkedNode))
            .expect("unregister must release capacity");
        scheduler.unregister(second);
    }
}
