use kithara_platform::{
    CancelGroup, CancelToken, CancelWakerGuard,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use kithara_test_macros as kithara;

use crate::{Priority, Task, TaskControl, TaskId};

pub(super) struct Reservation {
    capacity: Arc<Capacity>,
}

pub(super) struct Capacity {
    active: AtomicUsize,
    pub(super) limit: usize,
}

impl Capacity {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            active: AtomicUsize::new(0),
            limit,
        }
    }

    pub(super) fn reserve(capacity: &Arc<Self>) -> Option<Reservation> {
        capacity
            .active
            .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < capacity.limit).then_some(current + 1)
            })
            .ok()
            .map(|_| Reservation {
                capacity: Arc::clone(capacity),
            })
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.capacity.active.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) enum Command {
    Register(Slot),
    Unregister(TaskId),
    Shutdown,
}

pub(super) struct Slot {
    pub(super) _cancel_guards: Vec<CancelWakerGuard>,
    pub(super) cancel: CancelGroup,
    pub(super) control: TaskControl,
    pub(super) id: TaskId,
    pub(super) is_terminal: bool,
    pub(super) priority: Priority,
    pub(super) task: Box<dyn Task>,
    pub(super) token: CancelToken,
}

impl Slot {
    #[kithara::probe(task_id = self.id.get(), already_terminal = self.is_terminal)]
    pub(super) fn cancel(&mut self) {
        if self.is_terminal {
            return;
        }
        self.is_terminal = true;
        self.token.cancel();
        self.task.on_cancel();
        self.task.recycle();
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Clone, Copy)]
pub(super) struct SchedulerBudgets {
    pub(super) fairness_yield_interval: u32,
    pub(super) idle_timeout: Duration,
    pub(super) slow_tick_threshold: Duration,
    pub(super) task_burst: u32,
    pub(super) wait_timeout: Duration,
}
