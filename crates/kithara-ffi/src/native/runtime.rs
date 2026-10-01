use std::{sync::LazyLock, thread};

use kithara::platform::tokio::runtime::{self, Builder as RuntimeBuilder};

/// Shared tokio runtime handle for FFI background tasks (event bridges, polling).
///
/// Runs a single-threaded tokio runtime on a dedicated OS thread.
/// Only requires the `rt` feature (no `rt-multi-thread`), compatible with iOS.
pub(crate) static FFI_RUNTIME: LazyLock<runtime::Handle> = LazyLock::new(|| {
    let rt = RuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .expect("BUG: tokio current-thread runtime build cannot fail in normal startup");
    let handle = rt.handle().clone();
    // This process-wide runtime outlives its callers; each task carries its own context.
    thread::spawn(move || {
        rt.block_on(std::future::pending::<()>());
    });
    handle
});
