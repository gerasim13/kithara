use std::sync::OnceLock;

use kithara_platform::{
    sync::{Arc, ThreadGate, WaitGate},
    time::Duration,
};
use kithara_stream::{DeferredWake, WorkerWake};

/// Late-bound audio-worker wake shared by HLS readiness producers.
pub(crate) type WorkerWakeCell = Arc<OnceLock<Arc<dyn WorkerWake>>>;

fn wake_worker(cell: &WorkerWakeCell) {
    if let Some(wake) = cell.get() {
        wake.wake();
    }
}

/// Shared readiness signal for the reader, audio worker, and HLS peer.
#[derive(Clone)]
pub(crate) struct SizeSignal {
    /// Late-bound peer-poll wake.
    peer_wake: Arc<OnceLock<Arc<DeferredWake>>>,
    /// RT-safe gate used by the off-RT reader wait.
    ready: Arc<ThreadGate>,
    /// Late-bound audio-worker data-arrival wake.
    worker_wake: WorkerWakeCell,
}

impl SizeSignal {
    /// Create a signal over shared reader and worker wake handles.
    pub(crate) fn new(ready: Arc<ThreadGate>, worker_wake: WorkerWakeCell) -> Self {
        Self {
            ready,
            worker_wake,
            peer_wake: Arc::new(OnceLock::new()),
        }
    }

    delegate::delegate! {
        to self.ready {
            /// Snapshot the readiness generation before parking.
            pub(crate) fn current(&self) -> u64;
            /// Signal only the RT-safe reader gate.
            #[call(signal)]
            pub(crate) fn fire_ready_only(&self);
            /// Wait for a newer readiness generation until `timeout`.
            pub(crate) fn wait_timeout(&self, since: u64, timeout: Duration) -> bool;
        }
    }

    /// Signal the reader and audio worker.
    pub(crate) fn fire(&self) {
        self.ready.signal();
        wake_worker(&self.worker_wake);
    }

    /// Clone the underlying readiness gate.
    pub(crate) fn ready_gate(&self) -> Arc<ThreadGate> {
        Arc::clone(&self.ready)
    }

    /// Install the peer wake once.
    pub(crate) fn set_peer_wake(&self, wake: Arc<DeferredWake>) {
        let _ = self.peer_wake.set(wake);
    }

    /// Install the audio-worker wake once.
    pub(crate) fn set_worker_wake(&self, wake: Arc<dyn WorkerWake>) {
        let _ = self.worker_wake.set(wake);
    }

    /// Wake the HLS peer's `poll_next` so it re-runs `reconcile_escape` against
    /// the just-flagged stalled slot. Called from the `on_slow` hook on the
    /// downloader thread (off-RT — `notify_now`'s cross-thread `notify_one` is
    /// allowed here). A no-op until the peer activates; the stored-permit
    /// semantics mean a wake delivered between the peer's polls is not lost.
    pub(crate) fn wake_peer(&self) {
        if let Some(wake) = self.peer_wake.get() {
            wake.notify_now();
        }
    }
}
