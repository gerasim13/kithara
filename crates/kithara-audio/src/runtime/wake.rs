use std::sync::atomic::{AtomicU64, Ordering};

use kithara_platform::{
    sync::{ThreadGate, WaitGate},
    time::Duration,
};

use crate::runtime::WakeSignal;

/// Level-triggered scheduler wake over [`ThreadGate`]. The RT signal path uses
/// a sequence CAS, lock-free waiter snapshot, and `unpark`.
#[derive(Default)]
pub(crate) struct SchedulerWake {
    /// Gate counter consumed as of the previous wait (scheduler-thread only).
    seen: AtomicU64,
    gate: ThreadGate,
}

impl SchedulerWake {
    /// Block until [`wake`](Self::wake) fires or `timeout` elapses. Returns
    /// `true` if woken. Called only from the scheduler thread (single waiter).
    pub(crate) fn wait_timeout(&self, timeout: Duration) -> bool {
        let since = self.seen.load(Ordering::Relaxed);
        let woken = self.gate.wait_timeout(since, timeout);
        self.seen.store(self.gate.current(), Ordering::Relaxed);
        woken
    }

    /// Signal from any thread, including the real-time audio thread.
    pub(crate) fn wake(&self) {
        self.gate.signal();
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
        sync::Arc,
        thread::{self, spawn},
        time::Duration,
    };
    use kithara_test_utils::kithara;

    use super::ThreadWake;
    use crate::runtime::WakeSignal;

    #[kithara::test]
    fn wake_unparks_registered_thread() {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let wake = Arc::new(ThreadWake::default());
            let worker_wake = Arc::clone(&wake);

            let join = spawn(move || {
                let since = worker_wake.current();
                worker_wake.wait_timeout(since, Duration::from_secs(1))
            });

            thread::sleep(Duration::from_millis(10));
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
}
