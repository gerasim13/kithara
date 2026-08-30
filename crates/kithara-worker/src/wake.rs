use kithara_platform::{
    sync::{
        Arc, ThreadGate, WaitGate,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

/// Cloneable immediate and deferred scheduler wake capability.
#[derive(Clone, Default)]
pub struct Wake {
    inner: Arc<WakeInner>,
}

impl Wake {
    /// Wake the dispatcher immediately from an off-real-time thread.
    pub fn wake(&self) {
        self.inner.gate.signal();
    }

    /// Coalesce a future dispatcher pass without unparking the thread.
    pub fn defer(&self) {
        self.inner.deferred.store(true, Ordering::Release);
    }

    pub(crate) fn wait_timeout(&self, timeout: Duration) -> bool {
        if self.take_deferred() {
            return true;
        }
        let since = self.inner.seen.load(Ordering::Relaxed);
        let woken = self.inner.gate.wait_timeout(since, timeout);
        self.inner
            .seen
            .store(self.inner.gate.current(), Ordering::Relaxed);
        woken || self.take_deferred()
    }

    fn take_deferred(&self) -> bool {
        self.inner.deferred.swap(false, Ordering::Acquire)
    }
}

#[derive(Default)]
struct WakeInner {
    deferred: AtomicBool,
    gate: ThreadGate,
    seen: AtomicU64,
}

#[cfg(test)]
mod tests {
    use kithara_platform::time::Duration;
    #[cfg(not(target_arch = "wasm32"))]
    use kithara_platform::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::Instant,
    };
    use kithara_test_utils::kithara;

    use super::Wake;

    #[kithara::test(native, flash(false))]
    fn deferred_wake_is_level_triggered_and_coalesced() {
        let wake = Wake::default();
        wake.defer();
        wake.defer();

        assert!(wake.wait_timeout(Duration::ZERO));
        assert!(!wake.wait_timeout(Duration::ZERO));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[kithara::test(native, flash(false))]
    fn deferred_wake_publishes_preceding_work() {
        let wake = Wake::default();
        let published = Arc::new(AtomicUsize::new(0));
        let producer_wake = wake.clone();
        let producer_value = Arc::clone(&published);
        let producer = thread::spawn(move || {
            producer_value.store(42, Ordering::Relaxed);
            producer_wake.defer();
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        while !wake.wait_timeout(Duration::ZERO) {
            assert!(Instant::now() < deadline, "deferred wake was not observed");
            thread::yield_now();
        }
        assert_eq!(published.load(Ordering::Relaxed), 42);
        producer.join().expect("producer must not panic");
    }
}
