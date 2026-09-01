use std::{
    panic::Location,
    task::{Wake, Waker},
};

use super::{
    FLASH, credit,
    state::{AtomicTaskState, ParkOutcome, TaskDiag, TaskState, WakeOutcome},
};
use crate::{sync::Arc, system::lock::Mutex};

/// Per-task gate for quiescence accounting. It tracks whether the task currently
/// occupies an `active_async` slot and INTERCEPTS every wake (it is handed to the
/// inner future as its `Waker`), so a task that has been woken — its waker fired
/// and it is queued to be polled — is counted from that instant until it is next
/// polled. This closes the wake→poll window the old per-poll wrapper left open
/// (a runnable-but-not-yet-repolled task was uncounted, so the clock could jump
/// past it). Because the gate IS the inner future's waker, EVERY wake routes
/// through it: engine wakes, the real-I/O reactor, `JoinHandle`, raw channels.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(in crate::flash) struct TaskGate {
    #[field(get(vis = "pub(in crate::flash)", doc = "Returns this task's spawn site."))]
    loc: &'static Location<'static>,
    /// This task's FSM cell and poll count, shared with the engine's registry so
    /// a hang dump reads the live state — see [`TaskDiag`].
    diag: Arc<TaskDiag>,
    /// The runtime's waker for this task, refreshed each poll. The gate forwards
    /// to it on wake so the real poll is re-scheduled.
    runtime_waker: Mutex<Option<Waker>>,
    /// Stable engine identity of this task's `active_async` slot: the id minted by
    /// [`crate::flash::participate`] and the spawn-site [`Location`]. Every slot
    /// release ([`park`](Self::park) / [`complete`](Self::complete) /
    /// [`on_drop`](Self::on_drop)) and the wake re-acquire ([`Wake::wake_by_ref`])
    /// carry them, so the engine's holder map names WHICH task pins quiescence.
    #[field(get(
        vis = "pub(in crate::flash)",
        doc = "Returns this task's active_async slot id."
    ))]
    id: u64,
}

impl TaskGate {
    /// A fresh gate starts `Runnable`: a constructed/spawned task is queued to be
    /// polled, so it occupies a slot at once (acquired by
    /// [`crate::flash::participate`], which mints `id`, supplies the spawn `loc`,
    /// and keeps a second handle to `diag` in the registry).
    pub(super) fn new(id: u64, loc: &'static Location<'static>, diag: Arc<TaskDiag>) -> Arc<Self> {
        Arc::new(Self {
            id,
            loc,
            diag,
            runtime_waker: Mutex::default(),
        })
    }

    /// Poll returned `Ready`: the task is done — release its slot. The `DONE`
    /// store and the counter decrement happen together under the engine lock.
    pub(in crate::flash) fn complete(&self) {
        FLASH.gate_complete(self.state(), self.id);
    }

    /// A second handle to this task's diagnostics, for the engine's registry.
    pub(super) fn diag(&self) -> Arc<TaskDiag> {
        Arc::clone(&self.diag)
    }

    fn forward(&self) {
        let w = self.runtime_waker.lock().clone();
        if let Some(w) = w {
            w.wake();
        }
    }

    /// Drop: release the slot iff the task still occupies one (`RUNNABLE`/`RUNNING`/
    /// `RUNNING_NOTIFIED`). `PARKED` and `DONE` hold none.
    pub(in crate::flash) fn on_drop(&self) {
        FLASH.gate_drop_release(self.state(), self.id);
    }

    /// Poll returned `Pending`: `RUNNING`→`PARKED` releases the slot (a quiescent
    /// edge); a wake that landed mid-poll left `RUNNING_NOTIFIED`, so the CAS fails
    /// and the gate stays `RUNNABLE`, keeping the slot for the re-poll that wake
    /// already scheduled. The state transition and the counter move atomically
    /// under the engine lock so a concurrent wake cannot interleave — see
    /// [`super::FlashInner::gate_park`]. Both [`ParkOutcome`] arms are fully
    /// handled under that lock, so the returned outcome needs no action here.
    pub(in crate::flash) fn park(&self) {
        let _: ParkOutcome = FLASH.gate_park(self.state(), self.id);
    }

    /// This task's live FSM cell.
    fn state(&self) -> &AtomicTaskState {
        &self.diag.state
    }

    pub(in crate::flash) fn store_runtime_waker(&self, w: &Waker) {
        let mut g = self.runtime_waker.lock();
        match g.as_ref() {
            Some(existing) if existing.will_wake(w) => {}
            _ => *g = Some(w.clone()),
        }
    }

    /// Poll entry: claim the poll iff the task is `RUNNABLE` (it holds a slot and
    /// was genuinely queued). Returns `false` for any other state — a
    /// duplicate/stale schedule (the runtime can poll more times than there are
    /// wakes when several `forward`s race a `park`). The caller MUST then return
    /// `Pending` without polling the inner future or touching the slot, so the
    /// `active_async` accounting stays balanced. The slot is already held from
    /// spawn or the waking transition, so a successful claim changes no counter.
    pub(in crate::flash) fn try_enter_poll(&self) -> bool {
        let entered = self
            .state()
            .compare_exchange(TaskState::Runnable, TaskState::Running);
        if entered {
            self.diag.enter_poll(credit::current_thread_key());
        }
        entered
    }
}

impl Wake for TaskGate {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        loop {
            match self.state().load() {
                TaskState::Parked => {
                    // WHY: Re-acquire the slot the park released BEFORE the real poll runs: this wake->poll window must stay counted so the clock cannot
                    // jump past this task.
                    match FLASH.gate_wake_parked(self.state(), self.id, self.loc) {
                        WakeOutcome::Resumed => {
                            self.forward();
                            return;
                        }
                        WakeOutcome::NotParked => {}
                    }
                }
                TaskState::Running => {
                    if self
                        .state()
                        .compare_exchange(TaskState::Running, TaskState::RunningNotified)
                    {
                        // WHY: Woken during its own poll; slot already held. Forward so the runtime re-polls after the current poll returns.
                        self.forward();
                        return;
                    }
                }
                // WHY: Runnable / RunningNotified: already pending a poll, slot held - idempotent. Done: nothing to wake.
                TaskState::Runnable | TaskState::RunningNotified => {
                    self.forward();
                    return;
                }
                TaskState::Done => return,
            }
        }
    }
}
