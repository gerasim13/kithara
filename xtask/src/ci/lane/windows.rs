use std::env;

use anyhow::{Context, Result};

use crate::ci::process::{Process, require_os};

pub(crate) fn tests(process: &Process, target: &str) -> Result<()> {
    require_os("windows", "Windows")?;
    process.require_tools(&["cargo", "rustup", "sccache", "cmake", "ninja"])?;
    process.run(
        "rustup",
        &["target", "add", target],
        "install Windows target",
    )?;
    // Run this executable rather than `cargo xtask`, which would rebuild it.
    // Windows refuses to replace a running image, and Cargo reported that as
    // `failed to remove file target\debug\xtask.exe`. The binary is already
    // the one the job started, so there is nothing to rebuild.
    let xtask = env::current_exe().context("locating the running xtask executable")?;
    let mut command = process.command(&xtask);
    command
        .args([
            "test",
            "--lane=windows",
            "--profile",
            "ci",
            "--target",
            target,
        ])
        // `bungee-sys` builds through the `cmake` crate, which asks for the
        // Visual Studio generator and hard-codes `-Thost=x64` for every MSVC
        // target. This host is ARM64, so that picks the emulated x64 tools:
        // MSBuild then lost the tracker log it writes beside each object and
        // reported `MSB6003: link.exe could not be run`. Ninja is the one
        // generator the crate leaves alone, and it uses the compiler the
        // target already resolved.
        .env("CMAKE_GENERATOR", "Ninja");
    for (name, value) in fdk_flags(process, target)? {
        command.env(name, value);
    }
    process.run_command(&mut command, "Windows tests")
}

/// What libfdk-aac needs to compile for MSVC on ARM64.
///
/// `fdk-aac-sys` vendors a snapshot of the library from before it learned
/// `_M_ARM64`. Upstream maps that macro onto `__arm__` and `__ARM_ARCH_8__`;
/// without it the architecture chain in `FDK_archdef.h` runs off the end into
/// `#warning`, which MSVC rejects as `fatal error C1021` — with the conforming
/// preprocessor as well, measured on 14.44.35207. Supplying the two defines is
/// the same mapping upstream makes, and the shim covers the one GCC attribute
/// the snapshot also predates.
///
/// The x64 target needs none of this: that chain has recognised `_M_X64` all
/// along.
fn fdk_flags(process: &Process, target: &str) -> Result<Vec<(String, String)>> {
    if !target.starts_with("aarch64") {
        return Ok(Vec::new());
    }
    let shim = process.root().join(".config/windows/fdk-msvc-shim.h");
    let shim = shim
        .to_str()
        .with_context(|| format!("shim path is not UTF-8: {}", shim.display()))?;
    let flags = format!("-D__arm__ -D__ARM_ARCH_8__ -FI{shim}");
    // `cc` reads the per-target spelling with underscores.
    let suffix = target.replace('-', "_");
    Ok(vec![
        (format!("CFLAGS_{suffix}"), flags.clone()),
        (format!("CXXFLAGS_{suffix}"), flags),
    ])
}
