use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use kithara_platform::{
    sync::{ThreadGate, WaitGate},
    time::Duration,
};

use crate::runtime::WakeSignal;

/// Scheduler wake with immediate off-RT and deferred real-time signal paths.
#[derive(Default)]
pub(crate) struct SchedulerWake {
    /// Gate counter consumed as of the previous wait (scheduler-thread only).
    seen: AtomicU64,
    /// Coalesced RT request. Ring/control payloads own their synchronization.
    deferred: AtomicBool,
    gate: ThreadGate,
}

impl SchedulerWake {
    /// Block until an immediate or deferred wake fires, or `timeout` elapses.
    ///
    /// Called only from the scheduler thread (single waiter). A deferred wake
    /// that races with the park is observed after the bounded timeout.
    pub(crate) fn wait_timeout(&self, timeout: Duration) -> bool {
        if self.take_deferred() {
            return true;
        }
        let since = self.seen.load(Ordering::Relaxed);
        let woken = self.gate.wait_timeout(since, timeout);
        self.seen.store(self.gate.current(), Ordering::Relaxed);
        let deferred = self.take_deferred();
        woken || deferred
    }

    /// Signal and unpark the scheduler from an off-RT thread.
    pub(crate) fn wake(&self) {
        self.gate.signal();
    }

    /// Publish a coalesced wake without a syscall or thread unpark.
    pub(crate) fn defer(&self) {
        self.deferred.store(true, Ordering::Relaxed);
    }

    fn take_deferred(&self) -> bool {
        self.deferred.swap(false, Ordering::Relaxed)
    }
}

#[derive(Default)]
pub(crate) struct ThreadWake {
    gate: ThreadGate,
}

impl ThreadWake {
    delegate::delegate! {
        to self.gate {
            /// Snapshot the edge before checking the predicate the wait guards.
            pub(crate) fn current(&self) -> u64;
            /// Park until the edge moves past `since` or `timeout` elapses.
            pub(crate) fn wait_timeout(&self, since: u64, timeout: Duration) -> bool;
        }
    }
}

impl WakeSignal for ThreadWake {
    fn wake(&self) {
        self.gate.signal();
    }
}

#[cfg(test)]
mod tests {
    use kithara_platform::{
        sync::{Arc, WaitGate, mpsc},
        thread::spawn,
        time::Duration,
    };
    use kithara_test_utils::kithara;

    use super::{SchedulerWake, ThreadWake};
    use crate::runtime::WakeSignal;

    #[kithara::test(flash(false))]
    fn cross_thread_wake_after_snapshot_is_observed() {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let wake = Arc::new(ThreadWake::default());
            let worker_wake = Arc::clone(&wake);
            let (snapshot_tx, snapshot_rx) = mpsc::channel();

            let join = spawn(move || {
                let since = worker_wake.current();
                snapshot_tx.send(()).expect("report wake snapshot");
                worker_wake.wait_timeout(since, Duration::from_secs(1))
            });

            snapshot_rx.recv().expect("receive wake snapshot");
            wake.wake();
            assert!(join.join().expect("wake test thread"));
        }
    }

    #[kithara::test]
    fn wake_between_snapshot_and_wait_is_not_lost() {
        let wake = ThreadWake::default();
        let since = wake.current();
        wake.wake();

        assert!(wake.wait_timeout(since, Duration::ZERO));
    }

    #[kithara::test]
    fn scheduler_deferred_wake_is_level_triggered_and_coalesced() {
        let wake = SchedulerWake::default();
        let gate_epoch = wake.gate.current();
        wake.defer();
        wake.defer();

        assert_eq!(wake.gate.current(), gate_epoch);
        assert!(wake.wait_timeout(Duration::ZERO));
        assert!(!wake.wait_timeout(Duration::ZERO));
    }
}
