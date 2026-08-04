use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::ci::{
    config::CiConfig,
    process::{Process, require_os},
    run::PipelineKind,
};

/// Every Apple job repeats this preflight instead of inheriting it from a
/// predecessor. Each one is scheduled on its own and can be retried alone, so
/// the wrong Xcode has to fail in the job that would have used it.
fn preflight(process: &Process, config: &CiConfig) -> Result<()> {
    require_os("macos", "Apple")?;
    process.require_tools(&[
        "cargo",
        "just",
        "sccache",
        "swift",
        "xcodebuild",
        "xcodegen",
    ])?;
    let version = process.capture("xcodebuild", &["-version"], "xcodebuild -version")?;
    let actual = version
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("Xcode "))
        .context("xcodebuild -version did not report an Xcode version")?;
    if actual != config.pins.expected_xcode_version {
        bail!(
            "Xcode {} is required, found {actual}",
            config.pins.expected_xcode_version
        );
    }
    Ok(())
}

pub(crate) fn lint(process: &Process, config: &CiConfig) -> Result<()> {
    preflight(process, config)?;
    process.run("just", &["lint", "full"], "full lint gate")
}

pub(crate) fn msrv(process: &Process, config: &CiConfig) -> Result<()> {
    preflight(process, config)?;
    let mut check = process.command("just");
    check
        .env("RUSTUP_TOOLCHAIN", &config.pins.msrv_toolchain)
        .args(["check", "workspace"]);
    process.run_command(&mut check, "MSRV workspace check")
}

pub(crate) fn test(process: &Process, config: &CiConfig, kind: PipelineKind) -> Result<()> {
    preflight(process, config)?;
    match kind {
        PipelineKind::MergeRequest | PipelineKind::Quarantine => process.run(
            "just",
            &["test", "run", "--profile", "ci"],
            "Apple merge-request tests",
        ),
        PipelineKind::Main | PipelineKind::Nightly | PipelineKind::Release => process.run(
            "just",
            &[
                "test",
                "run",
                "--flash=on",
                "--no-block=on",
                "--profile",
                "ci",
            ],
            "Apple flash and no-block gate",
        ),
        PipelineKind::Weekly => bail!("weekly pipelines do not run the Apple suite"),
    }
}

/// The off-flash suite owns its own target directory: the two gates differ by
/// feature, and sharing one directory makes each run rebuild what the other
/// just replaced.
pub(crate) fn test_flash_off(process: &Process, config: &CiConfig) -> Result<()> {
    preflight(process, config)?;
    let mut command = process.command("just");
    command.env("CARGO_TARGET_DIR", "target-flash-off").args([
        "test",
        "run",
        "--flash=off",
        "--profile",
        "ci",
    ]);
    process.run_command(&mut command, "Apple flash-off gate")
}

pub(crate) fn xcframework(process: &Process, config: &CiConfig) -> Result<()> {
    preflight(process, config)?;
    build_xcframework(process)
}

pub(crate) fn swift_test(process: &Process, config: &CiConfig, swiftpm_cache: &Path) -> Result<()> {
    preflight(process, config)?;
    // The Swift package resolves the framework from the debug build tree, so
    // this job builds it too. Repeated work is nearly free — the jobs share a
    // target directory on the executor — and it keeps the job self-contained.
    build_xcframework(process)?;
    let mut command = process.command("swift");
    command
        .env("KITHARA_LOCAL_DEV", "1")
        .arg("test")
        .arg("--cache-path")
        .arg(swiftpm_cache);
    process.run_command(&mut command, "Swift package tests")
}

pub(crate) fn ios(process: &Process, config: &CiConfig) -> Result<()> {
    preflight(process, config)?;
    let mut command = process.command("just");
    command
        .env("KITHARA_LOCAL_DEV", "1")
        .args(["platform", "apple", "ios"]);
    process.run_command(&mut command, "iOS Simulator build")
}

pub(crate) fn safari(process: &Process) -> Result<()> {
    require_os("macos", "Safari WASM")?;
    process.require_tools(&["wasm-pack"])?;
    process.run(
        "wasm-pack",
        &["test", "tests", "--headless", "--safari"],
        "Safari WASM tests",
    )
}

fn build_xcframework(process: &Process) -> Result<()> {
    process.run(
        "just",
        &["platform", "apple", "xcframework", "--profile", "debug"],
        "Apple XCFramework",
    )
}
