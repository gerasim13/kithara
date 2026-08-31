use kithara_platform::time::Duration;

use crate::{Priority, TaskId, TickResult};

/// Best result from one pass over admitted tasks.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PassOutcome {
    /// At least one task made progress.
    Progress,
    /// At least one task waits without active upstream work.
    Waiting,
    /// At least one task has active upstream work.
    UpstreamPending,
    /// Live tasks are waiting for downstream capacity.
    Backpressured,
    /// No admitted task expects progress.
    Idle,
}

/// Allocation-free summary of one scheduler pass.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PassReport {
    pub active_tasks: usize,
    pub backpressured_tasks: usize,
    pub done_tasks: usize,
    pub first_backpressured_task: Option<TaskId>,
    pub first_progress_task: Option<TaskId>,
    pub first_upstream_pending_task: Option<TaskId>,
    pub first_waiting_priority: Option<Priority>,
    pub first_waiting_task: Option<TaskId>,
    pub outcome: PassOutcome,
    pub progress_tasks: usize,
    pub upstream_pending_tasks: usize,
    pub waiting_tasks: usize,
}

impl PassReport {
    pub(crate) const fn new(active_tasks: usize) -> Self {
        Self {
            active_tasks,
            backpressured_tasks: 0,
            done_tasks: 0,
            first_backpressured_task: None,
            first_progress_task: None,
            first_upstream_pending_task: None,
            first_waiting_priority: None,
            first_waiting_task: None,
            outcome: PassOutcome::Idle,
            progress_tasks: 0,
            upstream_pending_tasks: 0,
            waiting_tasks: 0,
        }
    }

    pub(crate) fn record(&mut self, id: TaskId, priority: Priority, result: TickResult) {
        match result {
            TickResult::Progress => {
                self.progress_tasks += 1;
                self.first_progress_task.get_or_insert(id);
            }
            TickResult::Waiting => {
                self.waiting_tasks += 1;
                self.first_waiting_task.get_or_insert(id);
                self.first_waiting_priority.get_or_insert(priority);
            }
            TickResult::UpstreamPending => {
                self.upstream_pending_tasks += 1;
                self.first_upstream_pending_task.get_or_insert(id);
            }
            TickResult::Backpressured => {
                self.backpressured_tasks += 1;
                self.first_backpressured_task.get_or_insert(id);
            }
            TickResult::Done => self.done_tasks += 1,
        }
    }
}

/// Observable scheduler lifecycle event.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    PassStart,
    PassEnd,
    Progress(PassReport),
    Idle(PassReport),
    Waiting(PassReport),
    UpstreamPending(PassReport),
    Backpressured(PassReport),
    SlowTick { task: TaskId, elapsed: Duration },
    TaskPanicked { task: TaskId },
}

/// Consumer of scheduler lifecycle events.
pub trait Observer: Send + 'static {
    fn on_event(&mut self, event: Event);
}
