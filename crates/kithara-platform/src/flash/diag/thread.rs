use std::backtrace::Backtrace;

use crate::flash::ids::ThreadKey;

/// Snapshot identity of an OS thread, captured for the hang dump. `key` matches
/// the engine's [`ThreadKey`] so a thread blocked acquiring a lock here can be
/// correlated with the same thread parked on an engine waiter.
#[derive(Clone, Debug, derive_more::Display)]
#[display("{name} [{id} {key:?}]", name = self.name.as_deref().unwrap_or("<unnamed>"), id = self.id, key = self.key)]
pub(in crate::flash) struct ThreadDesc {
    pub(in crate::flash) key: ThreadKey,
    name: Option<String>,
    id: String,
}

impl ThreadDesc {
    pub(in crate::flash) fn current() -> Self {
        let t = std::thread::current();
        Self {
            key: ThreadKey::of(t.id()),
            name: t.name().map(str::to_owned),
            id: format!("{:?}", t.id()),
        }
    }
}

/// The stack that is about to park on a waiter with no deadline, captured only
/// under `KITHARA_FLASH_SYNC_BT`.
///
/// Unlike [`current_thread_context`] this belongs to the WAITER, which is what a
/// hang dump needs: the snapshot is taken by a watchdog, never by the code that
/// parked. Callers capture it BEFORE taking the engine lock — a stack walk under
/// `core` would hold the whole engine for its duration.
pub(in crate::flash) fn parked_backtrace() -> Option<Backtrace> {
    super::bt_enabled().then(Backtrace::force_capture)
}

/// Context of the thread taking the dump: its identity plus, when
/// `KITHARA_FLASH_SYNC_BT=1`, the dump caller's full backtrace. A watchdog
/// caller is not necessarily the thread holding or waiting on a primitive —
/// for that, see [`parked_backtrace`].
pub(in crate::flash) fn current_thread_context() -> String {
    let desc = ThreadDesc::current();
    if super::bt_enabled() {
        format!(
            "current thread: {desc}\nbacktrace:\n{bt}",
            bt = Backtrace::force_capture(),
        )
    } else {
        format!("current thread: {desc}  (set KITHARA_FLASH_SYNC_BT=1 for a backtrace)")
    }
}
