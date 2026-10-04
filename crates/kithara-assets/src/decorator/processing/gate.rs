use kithara_platform::{
    CancelToken,
    sync::{Arc, CondvarGate},
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
enum Readiness {
    Pending,
    Ready,
    Failed,
}

pub(super) struct ReadinessGate {
    gate: CondvarGate<Readiness>,
    /// Backstop between condvar wakeups, so readiness or an abort request from
    /// the underlying resource is noticed without a shared writer gate. Mirrors
    /// `AssetStore::builder(pools).processing_gate_poll_interval(..)`.
    poll_interval: Duration,
}

impl ReadinessGate {
    pub(super) fn new(initial: bool, poll_interval: Duration) -> Self {
        Self {
            poll_interval,
            gate: CondvarGate::new(if initial {
                Readiness::Ready
            } else {
                Readiness::Pending
            }),
        }
    }

    pub(super) fn fail(&self) {
        *self.gate.lock() = Readiness::Failed;
        self.gate.notify_all();
    }

    pub(super) fn is_ready(&self) -> bool {
        matches!(*self.gate.lock(), Readiness::Ready)
    }

    pub(super) fn mark_ready(&self) -> bool {
        let mut state = self.gate.lock();
        if matches!(*state, Readiness::Failed) {
            return false;
        }
        *state = Readiness::Ready;
        drop(state);
        self.gate.notify_all();
        true
    }

    pub(super) fn wait_until_ready(
        &self,
        is_ready: &dyn Fn() -> bool,
        should_abort: &dyn Fn() -> bool,
    ) -> bool {
        let mut state = self.gate.lock();
        loop {
            match *state {
                Readiness::Failed => return false,
                Readiness::Ready => return true,
                Readiness::Pending => {}
            }
            if is_ready() {
                *state = Readiness::Ready;
                self.gate.notify_all();
                return true;
            }
            if should_abort() {
                return false;
            }
            let deadline = Instant::now() + self.poll_interval;
            state = self.gate.wait_until(state, deadline);
        }
    }

    pub(super) fn wait_until_ready_with_cancel(
        self: &Arc<Self>,
        cancel: &CancelToken,
        is_ready: &dyn Fn() -> bool,
        should_abort: &dyn Fn() -> bool,
    ) -> bool {
        let gate = Arc::clone(self);
        let _cancel_wake = cancel.on_cancel(move || {
            let _guard = gate.gate.lock();
            gate.gate.notify_all();
        });
        self.wait_until_ready(is_ready, &|| cancel.is_cancelled() || should_abort())
    }
}
