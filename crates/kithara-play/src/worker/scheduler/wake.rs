use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use kithara_platform::{
    sync::{ThreadGate, WaitGate},
    time::Duration,
};

/// Playback scheduler wake with immediate and deferred signal paths.
#[derive(Default)]
pub(crate) struct SchedulerWake {
    seen: AtomicU64,
    deferred: AtomicBool,
    gate: ThreadGate,
}

impl SchedulerWake {
    pub(super) fn wait_timeout(&self, timeout: Duration) -> bool {
        if self.take_deferred() {
            return true;
        }
        let since = self.seen.load(Ordering::Relaxed);
        let woken = self.gate.wait_timeout(since, timeout);
        self.seen.store(self.gate.current(), Ordering::Relaxed);
        let deferred = self.take_deferred();
        woken || deferred
    }

    pub(super) fn wake(&self) {
        self.gate.signal();
    }

    pub(super) fn defer(&self) {
        self.deferred.store(true, Ordering::Relaxed);
    }

    fn take_deferred(&self) -> bool {
        self.deferred.swap(false, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use kithara_platform::{sync::WaitGate, time::Duration};
    use kithara_test_utils::kithara;

    use super::SchedulerWake;

    #[kithara::test]
    fn deferred_wake_is_level_triggered_and_coalesced() {
        let wake = SchedulerWake::default();
        let gate_epoch = wake.gate.current();
        wake.defer();
        wake.defer();

        assert_eq!(wake.gate.current(), gate_epoch);
        assert!(wake.wait_timeout(Duration::ZERO));
        assert!(!wake.wait_timeout(Duration::ZERO));
    }
}
