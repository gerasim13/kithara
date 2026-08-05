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
        .args(["test", "--profile", "ci", "--target", target])
        // `bungee-sys` builds through the `cmake` crate, which asks for the
        // Visual Studio generator and hard-codes `-Thost=x64` for every MSVC
        // target. This host is ARM64, so that picks the emulated x64 tools:
        // MSBuild then lost the tracker log it writes beside each object and
        // reported `MSB6003: link.exe could not be run`. Ninja is the one
        // generator the crate leaves alone, and it uses the compiler the
        // target already resolved.
        .env("CMAKE_GENERATOR", "Ninja");
    process.run_command(&mut command, "Windows tests")
}
