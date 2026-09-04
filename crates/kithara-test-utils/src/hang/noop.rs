use std::{marker::PhantomData, path::PathBuf};

use kithara_platform::time::Duration;

pub trait HangDump {
    fn dump_json(&self) -> String;
    fn label(&self) -> Option<&str> {
        None
    }
}

impl<T> HangDump for T {
    fn dump_json(&self) -> String {
        String::new()
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoContext;

pub struct HangDetector<C: HangDump = NoContext>(PhantomData<C>);

/// Stress-only pre-kill timer used by `#[kithara::test]` expansions.
#[doc(hidden)]
#[must_use]
#[derive(Debug)]
pub struct PreKillGuard {
    _private: (),
}

impl PreKillGuard {
    /// Construct the inert no-`hang` counterpart of the stress timer.
    pub const fn new(_test_name: &str) -> Self {
        Self { _private: () }
    }
}

/// Preserve the pre-existing tracing-only Flash report when `hang` is off.
#[doc(hidden)]
pub fn record_test_hang(label: &str, _diagnostic: &str) {
    kithara_platform::flash::log_hang_dump(label);
}

/// No dump machinery compiled in: unexpected panics keep the default hook.
pub fn install_panic_dump() {}

/// See [`install_panic_dump`]: nothing records, so nothing to suppress.
#[doc(hidden)]
pub fn suppress_expected_panic_dumps() {}

impl<C: HangDump> HangDetector<C> {
    #[inline(always)]
    #[must_use]
    pub fn new(_label: &'static str, _timeout: Duration) -> Self {
        Self(PhantomData)
    }

    /// No watchdog compiled in: bound an event wait by the fallback timeout
    /// rather than below it, so a no-`hang` build still re-checks instead of
    /// blocking forever on a lost wakeup. (`&mut self` mirrors the real
    /// detector, whose lazy deadline stamp needs it.)
    #[inline(always)]
    #[must_use]
    pub fn remaining(&mut self) -> Duration {
        default_timeout()
    }

    #[inline(always)]
    pub fn reset(&mut self) {}

    #[inline(always)]
    pub fn reset_from(&mut self, _file: &'static str, _line: u32) {}

    /// No watchdog compiled in: the context closure is dropped **uncalled**, so
    /// the app-context collection never executes in a no-`hang` (release) build —
    /// a semantic guarantee independent of the optimizer.
    #[inline(always)]
    pub fn reset_with<F: FnOnce() -> C>(&mut self, _ctx_fn: F) {}

    #[inline(always)]
    pub fn reset_with_from<F: FnOnce() -> C>(
        &mut self,
        _ctx_fn: F,
        _file: &'static str,
        _line: u32,
    ) {
    }

    #[inline(always)]
    pub fn tick(&mut self) {}

    #[inline(always)]
    pub fn tick_from(&mut self, _file: &'static str, _line: u32) {}

    /// See [`reset_with`](Self::reset_with): the closure is dropped uncalled.
    #[inline(always)]
    pub fn tick_with<F: FnOnce() -> C>(&mut self, _ctx_fn: F) {}

    #[inline(always)]
    pub fn tick_with_from<F: FnOnce() -> C>(
        &mut self,
        _ctx_fn: F,
        _file: &'static str,
        _line: u32,
    ) {
    }

    #[inline(always)]
    #[must_use]
    pub fn with_dump_dir(self, _dir: PathBuf) -> Self {
        self
    }
}

#[must_use]
pub fn default_timeout() -> Duration {
    // xtask-lint-ignore: retry_fallback
    const FALLBACK_SECS: u64 = 10;
    Duration::from_secs(FALLBACK_SECS)
}

/// No watchdog compiled in, so there is no budget to shorten: the guard exists
/// only so a test that asks for a tighter one still compiles.
#[derive(Debug)]
#[must_use = "the override lasts only while this guard is alive"]
pub struct TimeoutOverride;

/// See [`override_timeout`](super::override_timeout) in the watchdog build.
#[inline(always)]
pub fn override_timeout(_timeout: Duration) -> TimeoutOverride {
    TimeoutOverride
}
