use std::task::{Context, Poll};

use kithara_render::{LaneTask, ServiceClass};
use kithara_platform::{CancelToken, CancelWakerGuard, sync::Arc};
use kithara_worker::{Priority, Task, TickResult};

/// The actual decoder lane retained by the dispatcher until release.
pub struct ResourceLane {
    task: Box<dyn LaneTask>,
    cancel: Option<CancelToken>,
    _cancel_link: Option<Arc<CancelWakerGuard>>,
}

impl ResourceLane {
    pub(crate) fn new(
        lane: impl LaneTask,
        cancel: Option<CancelToken>,
        cancel_link: Option<Arc<CancelWakerGuard>>,
    ) -> Self {
        Self { task: Box::new(lane), cancel, _cancel_link: cancel_link }
    }
}

impl Drop for ResourceLane {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
    }
}

impl Task for ResourceLane {
    fn on_cancel(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
        self.task.on_cancel();
    }

    fn priority(&self) -> Option<Priority> {
        self.task.priority()
    }

    fn recycle(&mut self) {
        self.task.recycle();
    }

    fn tick(&mut self) -> TickResult {
        self.task.tick()
    }

    fn warm_up(&mut self) {
        self.task.warm_up();
    }
}

impl LaneTask for ResourceLane {
    fn set_priority(&mut self, class: ServiceClass) {
        self.task.set_priority(class);
    }

    fn poll_commands(&mut self, context: &mut Context<'_>) -> Poll<()> {
        self.task.poll_commands(context)
    }
}
