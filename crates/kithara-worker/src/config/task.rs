use std::num::NonZeroUsize;

use kithara_config::Config;
use kithara_platform::{CancelGroup, atomic::RelaxedAtomicU32, sync::Arc};

use crate::Priority;

/// Admission, cancellation, priority, and compute budget for one task.
#[non_exhaustive]
#[derive(Clone, Config, fieldwork::Fieldwork)]
#[config(builder(existing))]
#[fieldwork(opt_in, with)]
pub struct TaskConfig {
    #[config(value)]
    #[field(with)]
    pub(crate) max_compute_tasks: NonZeroUsize,
    #[config(skip = "composed into the task cancel group")]
    #[field(with, option_set_some)]
    pub(crate) cancel: Option<CancelGroup>,
    #[config(value(Priority, self.priority()))]
    priority: Arc<RelaxedAtomicU32>,
}

impl TaskConfig {
    /// Create a task with no additional cancel source and priority zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn priority(&self) -> Priority {
        Priority::new(self.priority.load())
    }

    pub(crate) fn set_priority(&self, priority: Priority) {
        self.priority.store(priority.get());
    }

    #[must_use]
    pub fn with_priority(mut self, priority: Priority) -> Self {
        self.priority = Arc::new(RelaxedAtomicU32::new(priority.get()));
        self
    }
}

impl Default for TaskConfig {
    fn default() -> Self {
        Self {
            cancel: None,
            max_compute_tasks: NonZeroUsize::MIN,
            priority: Arc::new(RelaxedAtomicU32::new(0)),
        }
    }
}
