use std::sync::atomic::{AtomicU64, Ordering};

use kithara_platform::time::Duration;
use serde::Serialize;

/// Context payload that a [`HangDetector`](super::HangDetector) serializes when
/// a hang fires.
pub trait HangDump {
    fn dump_json(&self) -> String;
    fn label(&self) -> Option<&str> {
        None
    }
}

impl<T: Serialize> HangDump for T {
    fn dump_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }
}

/// Default empty context for a detector that carries no payload.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoContext;

impl Serialize for NoContext {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_unit_struct("NoContext")
    }
}

/// A test's requested watchdog budget in nanoseconds; `0` means unset.
static OVERRIDE_NANOS: AtomicU64 = AtomicU64::new(0);

/// Restores the previous watchdog budget when dropped.
#[derive(Debug)]
#[must_use = "the override lasts only while this guard is alive"]
pub struct TimeoutOverride(u64);

impl Drop for TimeoutOverride {
    fn drop(&mut self) {
        OVERRIDE_NANOS.store(self.0, Ordering::Release);
    }
}

/// Shortens (or lengthens) the watchdog budget every [`default_timeout`] call
/// hands out while the returned guard lives.
///
/// A test needing a watchdog to fire faster than the ambient budget takes this
/// instead of writing `KITHARA_HANG_TIMEOUT_SECS`: mutating the process
/// environment while any other thread reads it is undefined behaviour, and a
/// mutex over the writers does not fix that — the readers are the other half of
/// the race, and most of them (`std`, dependencies) cannot be made to take a
/// lock. This override is one atomic, so it races with nothing.
///
/// A zero `timeout` is ignored (the ambient budget stays), since a watchdog that
/// has already expired the moment it is armed reports every wait as a hang.
pub fn override_timeout(timeout: Duration) -> TimeoutOverride {
    let nanos = u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX);
    let previous = OVERRIDE_NANOS.swap(nanos, Ordering::AcqRel);
    TimeoutOverride(previous)
}

/// Watchdog timeout: a test's [`override_timeout`] wins, else
/// `KITHARA_HANG_TIMEOUT_SECS` (native), else a built-in budget. Precedence of
/// optional configuration, not a recovery chain — each source is the deliberate
/// answer for its scope (one test, one run, everything else).
#[must_use]
pub fn default_timeout() -> Duration {
    // xtask-lint-ignore: retry_fallback
    const FALLBACK_TIMEOUT: Duration = Duration::from_secs(10);
    match OVERRIDE_NANOS.load(Ordering::Acquire) {
        0 => super::platform::env_timeout().unwrap_or(FALLBACK_TIMEOUT),
        nanos => Duration::from_nanos(nanos),
    }
}
