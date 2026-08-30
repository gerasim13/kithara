use kithara_platform::{
    CancelGroup, CancelToken,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    tokio::runtime::Handle,
};

use crate::{
    ComputeContext, ComputeRejected, Wake,
    compute::{Budget, ComputeRuntime},
};

/// Numeric scheduler priority. Higher values run first.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Priority(u32);

impl Priority {
    /// Create a numeric priority.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Return the numeric priority value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Stable identifier assigned when task capacity is reserved.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TaskId(u64);

impl TaskId {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the numeric identifier.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Result of one short scheduler task quantum.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TickResult {
    Progress,
    Waiting,
    Backpressured,
    UpstreamPending,
    Done,
}

/// Domain task executed in short quanta by one dispatcher thread.
pub trait Task: Send + 'static {
    /// Perform one short quantum of work.
    fn tick(&mut self) -> TickResult;

    /// Reclaim deferred resources outside the task tick.
    fn recycle(&mut self) {}

    /// Prepare dispatcher-thread-local state before the first tick.
    fn warm_up(&mut self) {}

    /// Release domain state after cancellation or panic.
    fn on_cancel(&mut self) {}
}

/// Cloneable priority and wake control for an admitted task.
#[derive(Clone)]
pub struct TaskControl {
    priority: Arc<AtomicU32>,
    token: CancelToken,
    wake: Wake,
}

impl TaskControl {
    pub(crate) fn new(priority: Priority, token: CancelToken, wake: Wake) -> Self {
        Self {
            priority: Arc::new(AtomicU32::new(priority.get())),
            token,
            wake,
        }
    }

    /// Return the current scheduler priority.
    #[must_use]
    pub fn priority(&self) -> Priority {
        Priority::new(self.priority.load(Ordering::Relaxed))
    }

    /// Publish a new priority and coalesce a scheduler pass.
    pub fn set_priority(&self, priority: Priority) {
        self.priority.store(priority.get(), Ordering::Relaxed);
        self.wake.defer();
    }

    /// Cancel only this task subtree and wake its dispatcher.
    pub fn cancel(&self) {
        self.token.cancel();
        self.wake.wake();
    }

    delegate::delegate! {
        to self.wake {
            /// Wake the dispatcher immediately from an off-real-time thread.
            pub fn wake(&self);
            /// Coalesce a future pass without unparking from the caller.
            pub fn defer(&self);
            #[call(clone)]
            pub(crate) fn wake_handle(&self) -> Wake;
        }
    }
}

/// Resources and cancellation lineage supplied to a task factory.
#[non_exhaustive]
#[derive(Clone)]
pub struct TaskContext {
    cancel: CancelGroup,
    compute: Arc<ComputeRuntime>,
    compute_budget: Arc<Budget>,
    control: TaskControl,
    runtime: Option<Handle>,
    token: CancelToken,
}

impl TaskContext {
    pub(crate) fn new(
        cancel: CancelGroup,
        compute: Arc<ComputeRuntime>,
        compute_budget: Arc<Budget>,
        control: TaskControl,
        runtime: Option<Handle>,
        token: CancelToken,
    ) -> Self {
        Self {
            cancel,
            compute,
            compute_budget,
            control,
            runtime,
            token,
        }
    }

    /// OR-composed task and domain cancellation sources.
    #[must_use]
    pub const fn cancel_group(&self) -> &CancelGroup {
        &self.cancel
    }

    /// Cloneable priority and wake control for this task.
    #[must_use]
    pub fn control(&self) -> TaskControl {
        self.control.clone()
    }

    /// Shared Tokio runtime handle configured on the worker.
    #[must_use]
    pub const fn runtime(&self) -> Option<&Handle> {
        self.runtime.as_ref()
    }

    /// Submit a compute job only when both task and worker budgets admit it.
    ///
    /// # Errors
    ///
    /// Returns the original payload with
    /// [`ComputeSubmitError::Unavailable`](crate::ComputeSubmitError::Unavailable)
    /// without a configured pool,
    /// [`ComputeSubmitError::Saturated`](crate::ComputeSubmitError::Saturated)
    /// when either in-flight limit is full, or
    /// [`ComputeSubmitError::Cancelled`](crate::ComputeSubmitError::Cancelled)
    /// after task cancellation.
    pub fn submit_compute<T, F>(&self, payload: T, job: F) -> Result<(), ComputeRejected<T>>
    where
        T: Send + 'static,
        F: FnOnce(ComputeContext, T) + Send + 'static,
    {
        self.compute.submit(
            &self.compute_budget,
            &self.token,
            self.control.wake_handle(),
            payload,
            job,
        )
    }

    /// Derived child token owned by this task.
    #[must_use]
    pub const fn token(&self) -> &CancelToken {
        &self.token
    }
}
