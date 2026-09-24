use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

pub use ::unimock::*;

/// Shared observation state for test doubles that only count calls.
#[derive(Clone, Default)]
pub struct CallCounter(Arc<AtomicUsize>);

impl CallCounter {
    #[must_use]
    pub fn get(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }

    pub fn record(&self) {
        self.0.fetch_add(1, Ordering::Release);
    }
}
