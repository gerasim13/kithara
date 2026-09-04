use std::sync::atomic::{AtomicUsize, Ordering};

use kithara_platform::sync::Arc;

use super::BudgetSnapshot;

#[derive(Clone, Debug)]
pub(super) struct BudgetCounter {
    inner: Arc<BudgetCounterInner>,
}

#[derive(Debug)]
struct BudgetCounterInner {
    current: AtomicUsize,
    limit: usize,
    peak: AtomicUsize,
}

impl BudgetCounter {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            inner: Arc::new(BudgetCounterInner {
                current: AtomicUsize::new(0),
                limit,
                peak: AtomicUsize::new(0),
            }),
        }
    }

    pub(super) fn current(&self) -> usize {
        self.inner.current.load(Ordering::Relaxed)
    }

    pub(super) fn limit(&self) -> usize {
        self.inner.limit
    }

    pub(super) fn peak(&self) -> usize {
        self.inner.peak.load(Ordering::Relaxed)
    }

    pub(super) fn same_counter(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub(super) fn try_acquire(&self, amount: usize) -> Result<(), BudgetSnapshot> {
        let mut current = self.current();
        loop {
            let Some(next) = current.checked_add(amount) else {
                return Err(BudgetSnapshot {
                    current,
                    limit: self.limit(),
                });
            };
            if next > self.limit() {
                return Err(BudgetSnapshot {
                    current,
                    limit: self.limit(),
                });
            }
            match self.inner.current.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.inner.peak.fetch_max(next, Ordering::Relaxed);
                    return Ok(());
                }
                Err(actual) => current = actual,
            }
        }
    }

    pub(super) fn release(&self, amount: usize, owner: &'static str) -> bool {
        let mut current = self.current();
        loop {
            let Some(next) = current.checked_sub(amount) else {
                tracing::error!(current, amount, owner, "buffer-pool accounting underflow");
                return false;
            };
            match self.inner.current.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
    }
}
