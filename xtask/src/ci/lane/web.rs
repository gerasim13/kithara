use anyhow::Result;

use crate::ci::process::{Process, require_os};

/// One browser per job. The two engines fail for unrelated reasons, and a
/// Firefox regression should not hide whether Chromium still passes.
pub(crate) fn chromium(process: &Process) -> Result<()> {
    require_os("linux", "Web browser")?;
    process.require_tools(&["chromium", "chromedriver", "wasm-pack"])?;
    process.run(
        "wasm-pack",
        &["test", "tests", "--headless", "--chrome"],
        "Chromium WASM tests",
    )
}

pub(crate) fn firefox(process: &Process) -> Result<()> {
    require_os("linux", "Web browser")?;
    process.require_tools(&["firefox", "geckodriver", "wasm-pack"])?;
    process.run(
        "wasm-pack",
        &["test", "tests", "--headless", "--firefox"],
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
