use kithara_platform::{
    sync::{ThreadGate, WaitGate},
    time::Duration,
};

use crate::runtime::WakeSignal;

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
        sync::{Arc, mpsc},
        thread::{self, spawn},
        time::Duration,
    };
    use kithara_test_utils::kithara;

    use super::ThreadWake;
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
    fn wake_releases_waiter_before_timeout() {
        let wake = Arc::new(ThreadWake::default());
        let signaller = Arc::clone(&wake);
        let since = wake.current();

        let join = spawn(move || {
            thread::sleep(Duration::from_millis(5));
            signaller.wake();
        });

        assert!(wake.wait_timeout(since, Duration::from_secs(1)));
        join.join().expect("wake signaller thread");
    }

    #[kithara::test]
    fn wake_between_snapshot_and_wait_is_not_lost() {
        let wake = ThreadWake::default();
        let since = wake.current();
        wake.wake();

        assert!(wake.wait_timeout(since, Duration::ZERO));
    }
}
