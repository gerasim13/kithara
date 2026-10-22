use kithara_platform::{
    sync::{
        Arc, ThreadGate, WaitGate,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// Cloneable immediate and deferred scheduler wake capability.
#[derive(Clone, Default)]
pub struct Wake {
    inner: Arc<WakeInner>,
}

impl Wake {
    fn wait(&self, timeout: Duration, poll: bool) -> bool {
        if self.inner.gate.take_posted() {
            return true;
        }
        let since = self.inner.seen.load(Ordering::Relaxed);
        let woken = if poll {
            self.inner.gate.wait_poll_timeout(since, timeout)
        } else {
            self.inner.gate.wait_timeout(since, timeout)
        };
        self.inner
            .seen
            .store(self.inner.gate.current(), Ordering::Relaxed);
        woken
    }

    delegate::delegate! {
        to self.inner.gate {
            /// Coalesce a future dispatcher pass without unparking the thread.
            #[call(post)]
            pub fn defer(&self);
            /// Wake the dispatcher immediately from an off-real-time thread.
            #[call(signal)]
            pub fn wake(&self);
        }
        to self {
            #[call(wait)]
            pub(crate) fn wait_timeout(&self, timeout: Duration, [false]) -> bool;
            #[call(wait)]
            pub(crate) fn wait_poll_timeout(&self, timeout: Duration, [true]) -> bool;
        }
    }
}

#[derive(Default)]
struct WakeInner {
    seen: AtomicU64,
    gate: ThreadGate,
}

/// Observe immediate gate signals and pending deferred work without consuming either.
#[cfg(any(test, feature = "mock"))]
#[must_use]
pub fn wake_state(wake: &Wake) -> (u64, bool) {
    let posted = wake.inner.gate.take_posted();
    if posted {
        wake.inner.gate.post();
    }
    (wake.inner.gate.current(), posted)
}

#[cfg(test)]
mod tests {
    use kithara_platform::time::Duration;
    use kithara_test_utils::kithara;

    use super::Wake;

    #[kithara::test(native, browser, flash(false))]
    fn deferred_wake_is_level_triggered_and_coalesced() {
        let wake = Wake::default();
        wake.defer();
        wake.defer();

        assert!(wake.wait_timeout(Duration::ZERO));
        assert!(!wake.wait_timeout(Duration::ZERO));
    }

    #[kithara::test(native, flash(false))]
    fn deferred_wake_publishes_preceding_work() {
        use kithara_platform::{
            sync::{
                Arc,
                atomic::{AtomicUsize, Ordering},
            },
            thread,
            time::WallInstant,
        };

        let wake = Wake::default();
        let published = Arc::new(AtomicUsize::new(0));
        let producer_wake = wake.clone();
        let producer_value = Arc::clone(&published);
        let producer = thread::spawn(move || {
            producer_value.store(42, Ordering::Relaxed);
            producer_wake.defer();
        });

        let deadline = WallInstant::now() + Duration::from_secs(2);
        while !wake.wait_timeout(Duration::ZERO) {
            assert!(
                WallInstant::now() < deadline,
                "deferred wake was not observed"
            );
            thread::yield_now();
        }
        assert_eq!(published.load(Ordering::Relaxed), 42);
        producer.join().expect("producer must not panic");
    }
}
