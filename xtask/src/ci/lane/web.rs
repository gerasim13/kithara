use anyhow::Result;

use crate::ci::process::{Process, require_os};

/// One browser per job. The two engines fail for unrelated reasons, and a
/// Firefox regression should not hide whether Chromium still passes.
///
/// Both go through the repository's own wasm recipe rather than driving
/// `wasm-pack` directly. The target rebuilds std for shared memory, which
/// needs the pinned nightly; `wasm-pack` drives Cargo with the default
/// toolchain, so std keeps its stock features and the linker rejects it.
pub(crate) fn chromium(process: &Process) -> Result<()> {
    require_os("linux", "Web browser")?;
    process.require_tools(&["chromium", "chromedriver", "just"])?;
    process.run(
        "just",
        &["platform", "wasm", "test", "chrome"],
        "Chromium WASM tests",
    )
}

pub(crate) fn firefox(process: &Process) -> Result<()> {
    require_os("linux", "Web browser")?;
    process.require_tools(&["firefox", "geckodriver", "just"])?;
    process.run(
        "just",
        &["platform", "wasm", "test", "firefox"],
        "Firefox WASM tests",
    )
}

pub(crate) fn size(process: &Process) -> Result<()> {
    require_os("linux", "WASM size")?;
    process.require_tools(&["cargo", "just"])?;
    process.run(
        "just",
        &["platform", "wasm", "size-check"],
        "WASM size check",
    )
}
