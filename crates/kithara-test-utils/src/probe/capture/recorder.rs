use std::sync::{
    Mutex, OnceLock, PoisonError,
    atomic::{AtomicUsize, Ordering},
};

use kithara_platform::{
    sync::Arc,
    thread::sleep as thread_sleep,
    time::{Duration, Instant, sleep},
};

use super::event::ProbeEvent;

pub(super) type SharedLog = Arc<Mutex<Vec<ProbeEvent>>>;

static GLOBAL_LOG: OnceLock<SharedLog> = OnceLock::new();
static ACTIVE_LEASES: AtomicUsize = AtomicUsize::new(0);

/// One `install()`'s claim on the global log. The count is mutated only
/// under the log's own lock, so a layer that re-checks [`capture_active`]
/// while holding that lock cannot push into a log the last lease just
/// released.
struct CaptureLease {
    log: SharedLog,
}

impl CaptureLease {
    fn new(log: SharedLog) -> Self {
        {
            let _guard = log.lock().unwrap_or_else(PoisonError::into_inner);
            ACTIVE_LEASES.fetch_add(1, Ordering::Relaxed);
        }
        Self { log }
    }
}

impl Drop for CaptureLease {
    fn drop(&mut self) {
        let mut log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
        if ACTIVE_LEASES.fetch_sub(1, Ordering::Relaxed) == 1 {
            // A fresh `Vec` rather than `clear()`: capacity is the retained
            // memory, and a probe-heavy run leaves tens of MB of it.
            *log = Vec::new();
        }
    }
}

/// Whether any `Recorder` is alive to read what the layer would record.
pub(crate) fn capture_active() -> bool {
    ACTIVE_LEASES.load(Ordering::Relaxed) != 0
}

/// Lazily allocate (or fetch) the process-wide probe log. Called by
/// [`test::init_tracing`](crate::test::init_tracing) when it
/// constructs the global subscriber, and by [`install`] when a test
/// requests a recorder. Never sets a global subscriber by itself.
pub(crate) fn shared_log() -> SharedLog {
    GLOBAL_LOG
        .get_or_init(|| Arc::new(Mutex::new(Vec::new())))
        .clone()
}

/// Snapshot handle anchored at the moment of installation.
///
/// Cross-test isolation relies on the **install-id task-local** that
/// `#[kithara::test]` enters before the test body runs: every probe
/// firing stamps the owning `install_id` into its tracing event, and
/// this recorder filters by that id. Tasks spawned via `tokio::spawn`
/// inside the test inherit the task-local automatically; orphan
/// tasks from a just-finished test (downloader on-complete, audio
/// worker draining its last buffer) carry the *previous* test's id
/// and fall out of the new recorder's snapshot — even though they
/// emit after the new test's `install()`.
///
/// Capture is lease-bound: the global log's contents and backing
/// allocation live exactly as long as at least one `Recorder`.
/// Concurrent recorders share the log, and it is released only when
/// the last recorder drops. The timestamp filter (`start_at`) handles
/// the small window between scope entry and recorder construction.
#[must_use]
pub fn install() -> Recorder {
    let log = shared_log();
    let lease = Arc::new(CaptureLease::new(log.clone()));
    let start_at = Instant::now();
    Recorder {
        start_at,
        log,
        install_id: crate::probe::current_install_id(),
        _lease: lease,
    }
}

/// Per-test snapshot handle.
#[derive(Clone, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub struct Recorder {
    _lease: Arc<CaptureLease>,
    #[field(get, copy)]
    start_at: Instant,
    log: SharedLog,
    install_id: u64,
}

impl Recorder {
    /// Filter snapshot by `probe = ...` discriminator.
    #[must_use]
    pub fn events_with_probe(&self, name: &str) -> Vec<ProbeEvent> {
        self.snapshot()
            .into_iter()
            .filter(|e| e.probe_name() == Some(name))
            .collect()
    }

    /// All events recorded since this `Recorder` was created.
    ///
    /// Filters by `install_id` first (excludes orphan-task events from
    /// prior tests that outlive their `Drop`) and `start_at` second
    /// (excludes anything emitted before the recorder was anchored —
    /// possible if the install-id counter was bumped before drain
    /// completed).
    #[must_use]
    pub fn snapshot(&self) -> Vec<ProbeEvent> {
        let log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
        log.iter()
            .filter(|e| e.u64("install_id") == Some(self.install_id) && e.at >= self.start_at)
            .cloned()
            .collect()
    }

    /// Block until a probe event matches `predicate` or `budget` elapses.
    ///
    /// Tests should drive their progress through this, not through
    /// time-based polling loops on `Audio::read` / `Stream::len()`.
    /// Each `#[kithara::probe]` site emits a `seq`-stamped event the
    /// moment the production code reaches it; `wait_for_probe` makes
    /// that arrival the test's clock instead of wall time.
    ///
    /// Returns the first event recorded by this recorder for which
    /// `predicate` returns `true`. Includes events that arrived *before*
    /// this call (so a probe that fires before the first poll isn't lost
    /// to a race). Returns `None` only when `budget` elapsed without a
    /// match.
    ///
    /// `budget` should be a real budget — fail the test if it elapses
    /// rather than masking a hang. Polls every 5 ms inside the budget.
    pub fn wait_for_probe<F>(&self, predicate: F, budget: Duration) -> Option<ProbeEvent>
    where
        F: Fn(&ProbeEvent) -> bool,
    {
        let deadline = Instant::now() + budget;
        loop {
            if let Some(evt) = self.snapshot().into_iter().find(|e| predicate(e)) {
                return Some(evt);
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread_sleep(Duration::from_millis(5));
        }
    }

    /// Async variant of [`Self::wait_for_probe`].
    ///
    /// Same semantics — but yields the runtime via `kithara_platform::time::sleep`
    /// instead of `std::thread::sleep`. Required for tests that drive
    /// async work between probe polls (HTTP fetches, peer schedulers,
    /// reader tasks) on a `current_thread` runtime: the blocking variant
    /// starves the runtime, preventing those tasks from progressing.
    pub async fn wait_for_probe_async<F>(
        &self,
        predicate: F,
        budget: Duration,
    ) -> Option<ProbeEvent>
    where
        F: Fn(&ProbeEvent) -> bool,
    {
        let deadline = Instant::now() + budget;
        loop {
            if let Some(evt) = self.snapshot().into_iter().find(|e| predicate(e)) {
                return Some(evt);
            }
            if Instant::now() >= deadline {
                return None;
            }
            sleep(Duration::from_millis(5)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, PoisonError};

    use tracing::{Level, subscriber::with_default};
    use tracing_subscriber::layer::SubscriberExt as _;

    use super::{capture_active, install, shared_log};
    use crate::probe::capture::probe_layer;

    static CAPTURE_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn emit_probe(name: &'static str, install_id: u64) {
        let subscriber = tracing_subscriber::registry().with(probe_layer());
        with_default(subscriber, || {
            tracing::event!(
                target: "capture_contract_probe",
                Level::INFO,
                probe = name,
                install_id = install_id,
            );
        });
    }

    fn log_len() -> usize {
        let log = shared_log();
        let log = log.lock().unwrap_or_else(PoisonError::into_inner);
        log.len()
    }

    fn log_capacity() -> usize {
        let log = shared_log();
        let log = log.lock().unwrap_or_else(PoisonError::into_inner);
        log.capacity()
    }

    #[test]
    fn inactive_layer_does_not_record() {
        let _guard = CAPTURE_TEST_LOCK
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        assert!(!capture_active());

        emit_probe("inactive", crate::probe::current_install_id());

        assert_eq!(log_len(), 0, "unleased firing must not be recorded");
    }

    #[test]
    fn active_recorder_captures_events() {
        let _guard = CAPTURE_TEST_LOCK
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let recorder = install();
        let clone = recorder.clone();

        emit_probe("active", recorder.install_id);
        assert_eq!(recorder.events_with_probe("active").len(), 1);

        drop(recorder);
        assert!(capture_active());
        emit_probe("clone", clone.install_id);
        assert_eq!(clone.events_with_probe("clone").len(), 1);
    }

    #[test]
    fn overlapping_recorders_keep_shared_log() {
        let _guard = CAPTURE_TEST_LOCK
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let first = install();
        let second = install();
        emit_probe("overlap", second.install_id);

        drop(first);

        assert!(capture_active());
        assert_eq!(second.events_with_probe("overlap").len(), 1);
    }

    #[test]
    fn last_recorder_releases_global_log() {
        let _guard = CAPTURE_TEST_LOCK
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let recorder = install();
        emit_probe("release", recorder.install_id);
        assert_eq!(recorder.events_with_probe("release").len(), 1);

        drop(recorder);

        assert!(!capture_active(), "last drop must end the lease");
        assert_eq!(log_len(), 0, "last drop must clear the log");
        assert_eq!(
            log_capacity(),
            0,
            "last drop must release the backing allocation, not just its length"
        );
    }
}
