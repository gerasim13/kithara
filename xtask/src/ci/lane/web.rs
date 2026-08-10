use anyhow::{Result, bail};

use crate::ci::{
    config::CiPins,
    process::{Process, require_os},
};

/// One browser per job. The two engines fail for unrelated reasons, and a
/// Firefox regression should not hide whether Chromium still passes.
///
/// Both go through the repository's own wasm recipe rather than driving
/// `wasm-pack` directly. Chromium owns the crate-level `WebCodecs` target;
/// Firefox retains the full integration suite, whose shared-memory standard
/// library build needs the pinned nightly.
pub(crate) fn chromium(process: &Process, pins: &CiPins) -> Result<()> {
    require_os("linux", "Web browser")?;
    process.require_tools(&["chromium", "chromedriver", "just"])?;
    require_version(process, "chromium", &pins.chrome_for_testing_version)?;
    require_version(process, "chromedriver", &pins.chrome_for_testing_version)?;
    process.run(
        "just",
        &["platform", "wasm", "test-webcodecs", "chrome"],
        "Chromium WebCodecs tests",
    )
}

fn require_version(process: &Process, tool: &str, expected: &str) -> Result<()> {
    let label = format!("read {tool} version");
    let actual = process.capture(tool, &["--version"], &label)?;
    if !actual.split_whitespace().any(|part| part == expected) {
        bail!("{tool} version mismatch: expected {expected}, got {actual}");
    }
    Ok(())
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
