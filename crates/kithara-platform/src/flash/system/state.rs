use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};

use crate::{
    backend::tokio::runtime::{Handle, RuntimeFlavor},
    flash::ids::ThreadKey,
};

/// Per-task quiescence states. The task occupies one `active_async` slot
/// while it is in any non-quiescent state ([`Runnable`](TaskState::Runnable),
/// [`Running`](TaskState::Running), [`RunningNotified`](TaskState::RunningNotified));
/// it releases the slot only on the transition to [`Parked`](TaskState::Parked),
/// on completion, or on drop.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TaskState {
    Parked = 0,
    Runnable = 1,
    Running = 2,
    RunningNotified = 3,
    Done = 4,
}

/// Typed atomic over [`TaskState`]: the gate FSM cell. The orderings are fixed
/// per operation (CAS `AcqRel`/`Acquire`, swap `AcqRel`, store `Release`, load
/// `Acquire`) — exactly the orderings the untyped `AtomicU8` sites used.
pub(super) struct AtomicTaskState(AtomicU8);

impl AtomicTaskState {
    fn new(initial: TaskState) -> Self {
        Self(AtomicU8::new(initial as u8))
    }

    /// CAS `current -> new`; `true` iff it transitioned.
    pub(super) fn compare_exchange(&self, current: TaskState, new: TaskState) -> bool {
        self.0
            .compare_exchange(
                current as u8,
                new as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub(super) fn load(&self) -> TaskState {
        Self::unpack(self.0.load(Ordering::Acquire))
    }

    pub(super) fn store(&self, new: TaskState) {
        self.0.store(new as u8, Ordering::Release);
    }

    pub(super) fn swap(&self, new: TaskState) -> TaskState {
        Self::unpack(self.0.swap(new as u8, Ordering::AcqRel))
    }

    fn unpack(v: u8) -> TaskState {
        match v {
            0 => TaskState::Parked,
            1 => TaskState::Runnable,
            2 => TaskState::Running,
            3 => TaskState::RunningNotified,
            4 => TaskState::Done,
            // WHY: Only `TaskState` discriminants are ever stored in the cell.
            _ => unreachable!("BUG: invalid TaskState discriminant {v}"),
        }
    }
}

/// Outcome of a gate park attempt ([`super::FlashInner::gate_park`]).
pub(super) enum ParkOutcome {
    /// `Running -> Parked`: the slot was released (a quiescent edge).
    Parked,
    /// A wake landed mid-poll (`RunningNotified`), so the CAS failed: the gate
    /// stays runnable, keeping the slot for the re-poll that wake scheduled.
    WokenMidPoll,
}

/// Outcome of waking a parked gate ([`super::FlashInner::gate_wake_parked`]).
pub(super) enum WakeOutcome {
    /// `Parked -> Runnable`: the slot was re-acquired; forward the runtime waker.
    Resumed,
    /// The state left `Parked` between the load and the CAS; the caller's wake
    /// loop re-reads and handles the current state lock-free.
    NotParked,
}

/// What a pinning task is doing, shared between its
/// [`TaskGate`](super::gate::TaskGate) and the engine's registry so a hang dump
/// can read it without reaching for the task.
///
/// The state cell is the gate's own FSM cell — the dump reads the same word the
/// transitions CAS. The poll count rises once per claimed poll, which is what
/// separates a task spinning through wake-poll-park from one stranded
/// `Runnable` by a wake whose re-poll never came: both pin the clock with
/// `active_async=1` and are otherwise indistinguishable in a dump.
pub(super) struct TaskDiag {
    pub(super) state: AtomicTaskState,
    /// The runtime that drove that last poll polls on exactly ONE thread
    /// (`current_thread`). Then [`driver`](Self::driver) is not merely the last
    /// thread to poll this task but the only thread that can deliver its next
    /// poll — which is what separates a task nothing can move from one waiting
    /// its turn. Sampled per poll, so it always describes the runtime that owes
    /// the poll, even for a task spawned onto a handle from another runtime.
    sole_poller: AtomicBool,
    /// Raw [`ThreadKey`] of the thread that entered this task's last poll,
    /// written lock-free at the gate. Meaningful only once `polls > 0`, which
    /// publishes it.
    driver: AtomicU64,
    polls: AtomicU64,
}

/// A task is `Runnable` from the moment it is spawned, before its first poll —
/// not the cell's zero value, so this cannot be derived.
impl Default for TaskDiag {
    fn default() -> Self {
        Self {
            state: AtomicTaskState::new(TaskState::Runnable),
            polls: AtomicU64::new(0),
            driver: AtomicU64::new(0),
            sole_poller: AtomicBool::new(false),
        }
    }
}

impl TaskDiag {
    /// The thread that entered this task's last poll, once it has been polled.
    pub(super) fn driver(&self) -> Option<ThreadKey> {
        (self.polls() > 0).then(|| ThreadKey::from(self.driver.load(Ordering::Relaxed)))
    }

    /// Record the thread claiming this poll and whether its runtime has any
    /// other thread that could deliver the next one. The count is bumped LAST
    /// (`Release`, paired with `polls()`'s `Acquire`), so a reader that sees a
    /// non-zero count sees the matching driver rather than a stale one.
    pub(super) fn enter_poll(&self, driver: ThreadKey) {
        self.driver.store(driver.raw(), Ordering::Relaxed);
        self.sole_poller
            .store(sole_poller_runtime(), Ordering::Relaxed);
        self.polls.fetch_add(1, Ordering::Release);
    }

    /// Polls this task has actually entered (claimed at the gate), not polls the
    /// runtime attempted.
    pub(super) fn polls(&self) -> u64 {
        self.polls.load(Ordering::Acquire)
    }

    /// True when this task holds its slot as `Runnable` — queued for a poll —
    /// while the one thread that could deliver that poll is itself inside a
    /// bridged wait. Nothing can move such a task: see
    /// [`Registry::pinning_async`](super::Registry::pinning_async).
    pub(super) fn stranded_behind(&self, bridged: &BTreeSet<ThreadKey>) -> bool {
        self.driver()
            .is_some_and(|driver| bridged.contains(&driver))
            && self.sole_poller.load(Ordering::Relaxed)
            && self.state.load() == TaskState::Runnable
    }
}

/// Whether the runtime driving the current poll polls its tasks on exactly ONE
/// thread. A `current_thread` runtime's tasks can only ever be polled by its
/// `block_on` thread, so while that thread blocks inside a poll (a bridged
/// wait) nothing can poll them — and a task the engine counts as pinning then
/// holds a clock only that blocked thread could release. Anything else — a
/// multi-thread runtime, or no runtime at all — reads as "more than one
/// poller", the pinning default: another worker may still deliver the poll.
fn sole_poller_runtime() -> bool {
    matches!(
        Handle::try_current().map(|h| h.runtime_flavor()),
        Ok(RuntimeFlavor::CurrentThread)
    )
}
