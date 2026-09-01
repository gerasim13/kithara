use std::num::{NonZeroU32, NonZeroUsize};

use kithara_platform::{CancelGroup, time::Duration};

use crate::{Observer, observer::Event};

/// Scheduler thread budgets and observer.
#[non_exhaustive]
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, with)]
pub struct DispatcherConfig {
    pub(crate) observer: Box<dyn Observer>,
    #[field(with)]
    pub(crate) idle_timeout: Duration,
    #[field(with)]
    pub(crate) slow_tick_threshold: Duration,
    #[field(with)]
    pub(crate) wait_timeout: Duration,
    #[field(with)]
    pub(crate) fairness_yield_interval: NonZeroU32,
    #[field(with)]
    pub(crate) task_burst: NonZeroU32,
    #[field(with)]
    pub(crate) capacity: NonZeroUsize,
    #[field(with, option_set_some)]
    pub(crate) cancel: Option<CancelGroup>,
    pub(crate) name: String,
}

impl DispatcherConfig {
    /// Create scheduler settings with conservative general-purpose budgets.
    #[must_use]
    pub fn new<N: Into<String>>(name: N) -> Self {
        Self {
            cancel: None,
            capacity: NonZeroUsize::new(64).unwrap_or(NonZeroUsize::MIN),
            fairness_yield_interval: NonZeroU32::new(16).unwrap_or(NonZeroU32::MIN),
            idle_timeout: Duration::from_millis(100),
            name: name.into(),
            observer: Box::new(NoopObserver),
            slow_tick_threshold: Duration::from_millis(10),
            task_burst: NonZeroU32::new(32).unwrap_or(NonZeroU32::MIN),
            wait_timeout: Duration::from_millis(10),
        }
    }

    /// Replace the no-op observer for this dispatcher.
    #[must_use]
    pub fn with_observer<O: Observer>(mut self, observer: O) -> Self {
        self.observer = Box::new(observer);
        self
    }
}

struct NoopObserver;

impl Observer for NoopObserver {
    fn on_event(&mut self, _event: Event) {}
}
