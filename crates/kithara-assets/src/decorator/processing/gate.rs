use std::sync::atomic::{AtomicBool, Ordering};

use kithara_platform::{
    CancelToken,
    sync::{Arc, CondvarGate},
    time::{Duration, Instant},
};

pub(super) struct ReadinessGate {
    failed: AtomicBool,
    gate: CondvarGate<bool>,
    /// Backstop between condvar wakeups, so an abort request is noticed even
    /// when no writer ever signals. Mirrors
    /// `AssetStore::builder(pools).processing_gate_poll_interval(..)`.
    poll_interval: Duration,
}

impl ReadinessGate {
    pub(super) fn new(initial: bool, poll_interval: Duration) -> Self {
        Self {
            poll_interval,
            gate: CondvarGate::new(initial),
            failed: AtomicBool::new(false),
        }
    }

    pub(super) fn fail(&self) {
        self.failed.store(true, Ordering::Release);
        self.gate.notify_all();
    }

    pub(super) fn is_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    pub(super) fn is_ready(&self) -> bool {
        *self.gate.lock()
    }

    pub(super) fn mark_ready(&self) {
        *self.gate.lock() = true;
        self.gate.notify_all();
    }

    pub(super) fn wait_until_ready(&self, should_abort: &dyn Fn() -> bool) -> bool {
        loop {
            if self.is_failed() {
                return false;
            }
            let ready = {
                let guard = self.gate.lock();
                if *guard {
                    return !self.is_failed();
                }
                if self.is_failed() || should_abort() {
                    return false;
                }
                let deadline = Instant::now() + self.poll_interval;
                let next = self.gate.wait_until(guard, deadline);
                *next
            };
            if ready {
                return !self.is_failed();
            }
            if self.is_failed() || should_abort() {
                return false;
            }
        }
    }

    pub(super) fn wait_until_ready_with_cancel(
        self: &Arc<Self>,
        cancel: &CancelToken,
        should_abort: &dyn Fn() -> bool,
    ) -> bool {
        let gate = Arc::clone(self);
        let _cancel_wake = cancel.on_cancel(move || {
            let _guard = gate.gate.lock();
            gate.gate.notify_all();
        });
        self.wait_until_ready(&|| cancel.is_cancelled() || should_abort())
    }
}
