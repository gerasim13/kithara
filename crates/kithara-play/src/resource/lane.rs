use std::task::{Context, Poll};

use kithara_render::{LaneTask, ServiceClass};
use kithara_worker::{Priority, Task, TickResult};

/// The actual decoder lane retained by the dispatcher until release.
pub struct ResourceLane(Box<dyn LaneTask>);

impl ResourceLane {
    pub(crate) fn new(lane: impl LaneTask) -> Self {
        Self(Box::new(lane))
    }
}

impl Task for ResourceLane {
    fn on_cancel(&mut self) {
        self.0.on_cancel();
    }

    fn priority(&self) -> Option<Priority> {
        self.0.priority()
    }

    fn recycle(&mut self) {
        self.0.recycle();
    }

    fn tick(&mut self) -> TickResult {
        self.0.tick()
    }

    fn warm_up(&mut self) {
        self.0.warm_up();
    }
}

impl LaneTask for ResourceLane {
    fn set_priority(&mut self, class: ServiceClass) {
        self.0.set_priority(class);
    }

    fn poll_commands(&mut self, context: &mut Context<'_>) -> Poll<()> {
        self.0.poll_commands(context)
    }
}
