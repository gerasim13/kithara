use kithara_platform::maybe_send::MaybeSend;
use kithara_worker::{PendingTask, Task, TaskContext, TaskControl, TaskError};

pub(crate) use kithara_worker::TaskHandle as OfflineTaskHandle;

#[derive(Clone)]
pub(crate) struct OfflineTaskRoute(TaskControl);

impl OfflineTaskRoute {
    pub(super) fn new(pending: &PendingTask) -> Self {
        Self(pending.context().control())
    }

    pub(super) fn start<T: Task>(
        self,
        pending: PendingTask,
        factory: impl FnOnce(TaskContext) -> T + MaybeSend + 'static,
    ) -> Result<OfflineTaskHandle, TaskError> {
        pending.start_local(factory)
    }

    pub(super) fn wake(&self) {
        self.0.wake();
    }
}
