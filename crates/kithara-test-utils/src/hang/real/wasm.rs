use std::path::Path;

use kithara_platform::time::Duration;

use super::shared::{HangDump, NoContext};

/// Native stress pre-kill timer counterpart. Browser runners use their own
/// termination model, so this guard is inert on wasm.
#[doc(hidden)]
#[must_use]
#[derive(Debug)]
pub struct PreKillGuard {
    _private: (),
}

impl PreKillGuard {
    /// Construct the inert browser counterpart of the native timer.
    #[must_use]
    pub const fn new(_test_name: &str) -> Self {
        Self { _private: () }
    }
}

/// Report a detected hang. wasm cannot panic (`panic = "immediate-abort"`
/// turns it into a fatal `RuntimeError: unreachable` that kills the Worker),
/// so the detector reports through [`kithara_platform::logging::log_error`], which
/// writes to the browser `console` and is safe in every scope (including the
/// audio worklet, where the global `tracing` subscriber's cross-instance
/// vtable would trap).
pub(crate) fn write_dump<C: HangDump>(label: &str, ctx: &C, _dir: Option<&Path>, diag: &str) {
    let flash = kithara_platform::flash::hang_dump(label);
    let flash = if flash.trim().is_empty() {
        String::new()
    } else {
        format!("\n{flash}")
    };
    kithara_platform::logging::log_error(&format!(
        "[kithara_hang_detector] hang detected: {label} [{diag}] - {}{flash}",
        ctx.dump_json()
    ));
}

/// Record attempt-correlated hang evidence for `#[kithara::test]` expansions.
#[doc(hidden)]
pub fn record_test_hang(label: &str, diagnostic: &str) {
    write_dump(label, &NoContext, None, diagnostic);
}

#[must_use]
pub(crate) fn env_timeout() -> Option<Duration> {
    None
}
