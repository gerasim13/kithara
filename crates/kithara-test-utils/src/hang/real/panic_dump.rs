use std::{
    cell::Cell,
    panic::{self, PanicHookInfo},
};

use kithara_platform::sync::OnceLock;
use serde::Serialize;

use super::platform::write_dump;

thread_local! {
    static SUPPRESS_NEXT: Cell<bool> = const { Cell::new(false) };
    static SUPPRESS_ALL: Cell<bool> = const { Cell::new(false) };
}

/// The panicking site already wrote its own envelope (hang watchdog fire,
/// wall/sync timeout); the very next panic on this thread must not duplicate
/// it as a second `panic` dump.
pub(crate) fn suppress_next_panic_dump() {
    SUPPRESS_NEXT.with(|cell| cell.set(true));
}

/// The running test expects a panic (`#[should_panic]`), so its panics are
/// the contract, not evidence. Called by the `#[kithara::test]` prelude.
#[doc(hidden)]
pub fn suppress_expected_panic_dumps() {
    SUPPRESS_ALL.with(|cell| cell.set(true));
}

#[derive(Serialize)]
struct PanicContext {
    recent_events: Vec<String>,
    recent_probes: Vec<String>,
}

/// Process-wide hook recording a `kithara.hang.v1` envelope for every
/// unexpected panic — assertion failures included, which otherwise die with
/// stdout as their only trace. The envelope carries the flight-recorder tail,
/// so a red case brings its own DEBUG context without a pre-arranged filter.
/// Idempotent; chains to the previously installed hook.
pub fn install_panic_dump() {
    static INSTALL: OnceLock<()> = OnceLock::new();
    INSTALL.get_or_init(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            record_panic(info);
            previous(info);
        }));
    });
}

fn record_panic(info: &PanicHookInfo<'_>) {
    let suppressed = SUPPRESS_ALL.with(Cell::get) || SUPPRESS_NEXT.with(|cell| cell.replace(false));
    if suppressed {
        return;
    }
    let location = info.location().map_or_else(
        || "<unknown>".to_owned(),
        |location| format!("{}:{}", location.file(), location.line()),
    );
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".to_owned());
    let context = PanicContext {
        recent_events: crate::flight::tail(),
        recent_probes: crate::flight::probes_tail(),
    };
    let diagnostic = format!("panic at {location}: {message}");
    write_dump("panic", &context, None, &diagnostic);
}
