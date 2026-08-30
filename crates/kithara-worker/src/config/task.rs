use std::num::NonZeroUsize;

use kithara_platform::CancelGroup;

use crate::Priority;

/// Admission, cancellation, priority, and compute budget for one task.
#[non_exhaustive]
#[derive(Clone, fieldwork::Fieldwork)]
#[fieldwork(with)]
pub struct TaskConfig {
    #[field(option_set_some)]
    pub(crate) cancel: Option<CancelGroup>,
    pub(crate) max_compute_tasks: NonZeroUsize,
    pub(crate) priority: Priority,
}

impl TaskConfig {
    /// Create a task with no additional cancel source and priority zero.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            cancel: None,
            max_compute_tasks: NonZeroUsize::MIN,
            priority: Priority::new(0),
        }
    }
}

impl Default for TaskConfig {
    fn default() -> Self {
        Self::new()
    }
}
