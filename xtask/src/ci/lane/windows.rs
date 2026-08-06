use std::{env, process::Command};

use anyhow::{Context, Result};

use crate::ci::process::{Process, require_os};

pub(crate) fn tests(process: &Process, target: &str) -> Result<()> {
    prepare(process, target)?;
    // Run this executable rather than `cargo xtask`, which would rebuild it.
    // Windows refuses to replace a running image, and Cargo reported that as
    // `failed to remove file target\debug\xtask.exe`. The binary is already
    // the one the job started, so there is nothing to rebuild.
    let xtask = env::current_exe().context("locating the running xtask executable")?;
    let mut command = process.command(&xtask);
    command.args(["test", "--profile", "ci", "--target", target]);
    windows_build_env(process, &mut command, target)?;
    process.run_command(&mut command, "Windows tests")
}

/// The `x86_64` suite used to run here too, on a host with no `x86_64` in
/// it: every test reported on the emulator as much as on the commit, and took
/// the time to say so. An x86 executor runs that lane on hardware. What this
/// host still answers for is whether the target compiles at all — the native
/// dependencies break per target, not per instruction executed — so it builds
/// the workspace and everything it tests with, and runs none of it.
pub(crate) fn build(process: &Process, target: &str) -> Result<()> {
    prepare(process, target)?;
    let mut command = process.command("cargo");
    command.args([
        "check",
        "--locked",
        "--workspace",
        "--all-targets",
        "--target",
        target,
    ]);
    windows_build_env(process, &mut command, target)?;
    process.run_command(&mut command, "Windows build")
}

fn prepare(process: &Process, target: &str) -> Result<()> {
    require_os("windows", "Windows")?;
    process.require_tools(&["cargo", "rustup", "sccache", "cmake", "ninja"])?;
    process.run(
        "rustup",
        &["target", "add", target],
        "install Windows target",
    )
}

fn windows_build_env(process: &Process, command: &mut Command, target: &str) -> Result<()> {
    // `bungee-sys` builds through the `cmake` crate, which asks for the Visual
    // Studio generator and hard-codes `-Thost=x64` for every MSVC target. This
    // host is ARM64, so that picks the emulated x64 tools: MSBuild then lost
    // the tracker log it writes beside each object and reported `MSB6003:
    // link.exe could not be run`. Ninja is the one generator the crate leaves
    // alone, and it uses the compiler the target already resolved.
    command.env("CMAKE_GENERATOR", "Ninja");
    for (name, value) in fdk_flags(process, target)? {
        command.env(name, value);
    }
    Ok(())
}

/// What libfdk-aac needs to compile for MSVC on ARM64.
///
/// `fdk-aac-sys` vendors a snapshot of the library from before it learned
/// `_M_ARM64` and before one GCC attribute became portable; the shim carries
/// both fixes and explains them. Everything lives in the header rather than in
/// defines here, because a flag for a target reaches every crate built for it
/// and the header can ask which compiler is reading it — `aws-lc-sys` picks
/// `clang-cl` for this target and needs none of this.
///
/// The x64 target needs none of it either: that architecture chain has
/// recognised `_M_X64` all along.
fn fdk_flags(process: &Process, target: &str) -> Result<Vec<(String, String)>> {
    if !target.starts_with("aarch64") {
        return Ok(Vec::new());
    }
    let shim = process.root().join(".config/windows/fdk-msvc-shim.h");
    let shim = shim
        .to_str()
        .with_context(|| format!("shim path is not UTF-8: {}", shim.display()))?;
    let flags = format!("-FI{shim}");
    // `cc` reads the per-target spelling with underscores.
    let suffix = target.replace('-', "_");
    Ok(vec![
        (format!("CFLAGS_{suffix}"), flags.clone()),
        (format!("CXXFLAGS_{suffix}"), flags),
    ])
}
