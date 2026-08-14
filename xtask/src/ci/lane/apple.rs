use std::{
    net::{SocketAddr, TcpStream},
    path::Path,
    process::Child,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

use crate::ci::{config::CiConfig, process::Process, run::PipelineKind, xcresult};

/// Every Apple job repeats this preflight instead of inheriting it from a
/// predecessor. Each one is scheduled on its own and can be retried alone, so
/// the wrong Xcode has to fail in the job that would have used it.
fn preflight(process: &Process, config: &CiConfig) -> Result<()> {
    process.require_os("macos", "Apple")?;
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

/// The gate a change is measured by, and the ratchets the default branch keeps.
///
/// Both chains run the checks that answer whether a change is acceptable:
/// formatting, Clippy over the workspace, ast-grep, typos, the architecture
/// ratchet and the two idiom checks a review is held to. What only the default
/// branch pays for is the rest of the ratchet family — the full idiom set,
/// style, the quality scans and this tool's own tests. They guard a baseline
/// rather than judge a diff, and on one Apple host every one of them is a
/// minute a review waits.
fn lint_command(kind: PipelineKind) -> (&'static [&'static str], &'static str) {
    match kind {
        PipelineKind::MergeRequest | PipelineKind::Branch | PipelineKind::Quarantine => {
            (&["lint", "fast"], "review lint gate")
        }
        PipelineKind::Main
        | PipelineKind::Platforms
        | PipelineKind::Nightly
        | PipelineKind::Weekly
        | PipelineKind::Release => (&["lint", "full"], "full lint gate"),
    }
}

pub(crate) fn lint(process: &Process, config: &CiConfig, kind: PipelineKind) -> Result<()> {
    preflight(process, config)?;
    let (args, label) = lint_command(kind);
    process.run("just", args, label)
}

pub(crate) fn msrv(process: &Process, config: &CiConfig) -> Result<()> {
    preflight(process, config)?;
    let mut check = process.command("just");
    check
        .env("RUSTUP_TOOLCHAIN", &config.pins.msrv_toolchain)
        .args(["check", "workspace"]);
    process.run_command(&mut check, "MSRV workspace check")
}

fn test_command(kind: PipelineKind) -> Result<(&'static [&'static str], &'static str)> {
    match kind {
        PipelineKind::Quarantine => Ok((
            &["test", "run", "--profile", "ci"],
            "Apple quarantine tests",
        )),
        // A review ref asks the same question the default branch asks: what a
        // reviewer would have to fix anyway is cheaper to learn before merge.
        PipelineKind::MergeRequest
        | PipelineKind::Branch
        | PipelineKind::Main
        | PipelineKind::Platforms
        | PipelineKind::Nightly
        | PipelineKind::Release => Ok((
            &[
                "test",
                "run",
                "--flash=on",
                "--no-block=on",
                "--profile",
                "ci",
            ],
            "Apple flash and no-block gate",
        )),
        PipelineKind::Weekly => bail!("weekly pipelines do not run the Apple suite"),
    }
}

pub(crate) fn test(process: &Process, config: &CiConfig, kind: PipelineKind) -> Result<()> {
    preflight(process, config)?;
    let (args, label) = test_command(kind)?;
    process.run("just", args, label)
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

/// End-to-end playback across the resampler axis. Each lane pins one
/// source-rate conversion shape, and a change to decode, blending or output can
/// break one of them while every unit test still passes — so the three run
/// together or the axis is not covered. They share a target directory: the
/// lanes differ by feature, and the executor keeps the tree warm between them.
pub(crate) fn e2e(process: &Process, config: &CiConfig) -> Result<()> {
    preflight(process, config)?;
    for lane in ["--lane=e2e", "--lane=e2e-fused", "--lane=e2e-glide"] {
        process.run(
            "just",
            &["test", "run", lane, "--profile", "ci"],
            "Apple end-to-end suite",
        )?;
    }
    Ok(())
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
    // SwiftPM writes xUnit on request, so this lane needs no conversion.
    let report = process.root().join("target/xcresult/swift-test.junit.xml");
    if let Some(parent) = report.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut command = process.command("swift");
    command
        .env("KITHARA_LOCAL_DEV", "1")
        .arg("test")
        .arg("--cache-path")
        .arg(swiftpm_cache)
        .arg("--xunit-output")
        .arg(&report);
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

/// The `XCTest` suite on a simulator. It reads its fixtures from a hermetic
/// server rather than from the network, and nothing else in the job starts
/// one, so this lane owns its lifetime: the guard stops the server however the
/// job ends, because one left holding the port fails the next run with nothing
/// more informative than a bind error.
pub(crate) fn ios_test(process: &Process, config: &CiConfig) -> Result<()> {
    preflight(process, config)?;
    let server = TestServer::start(process)?;
    let mut command = process.command("just");
    command
        .env("KITHARA_LOCAL_DEV", "1")
        .env("KITHARA_TEST_SERVER_URL", server.url())
        // The framework comes from the job that builds it. This one holds the
        // measured group while a simulator runs, and rebuilding what another
        // job already produced spends that window twice.
        .args(["platform", "apple", "test", "--skip-build"]);
    let outcome = process.run_command(&mut command, "iOS Simulator tests");
    // A failing run is exactly the one whose report matters, so the bundle is
    // converted either way and the test outcome is returned afterwards.
    let bundle = process.root().join("target/xcresult/ios-test.xcresult");
    if bundle.exists() {
        xcresult::write_junit(
            process,
            &bundle,
            &process.root().join("target/xcresult/ios-test.junit.xml"),
        )?;
    }
    outcome
}

struct Consts;

impl Consts {
    /// The simulator shares the host network stack, so it reaches the server
    /// over loopback. The port is fixed because Apple simulator suites are
    /// serialized on the host, so another CI lane cannot bind it concurrently.
    const TEST_SERVER_PORT: u16 = 3444;
    const TEST_SERVER_POLL: Duration = Duration::from_millis(200);
    const TEST_SERVER_READY: Duration = Duration::from_secs(60);
}

struct TestServer {
    child: Child,
    url: String,
}

impl TestServer {
    fn start(process: &Process) -> Result<Self> {
        process.run(
            "cargo",
            &[
                "build",
                "-p",
                "kithara-integration-tests",
                "--bin",
                "test_server",
            ],
            "hermetic test server",
        )?;
        let binary = process.target_dir().join("debug/test_server");
        let mut command = process.command(&binary);
        command.env("TEST_SERVER_PORT", Consts::TEST_SERVER_PORT.to_string());
        let child = command
            .spawn()
            .with_context(|| format!("starting {}", binary.display()))?;
        let server = Self {
            child,
            url: format!("http://127.0.0.1:{}", Consts::TEST_SERVER_PORT),
        };
        wait_until_listening()?;
        Ok(server)
    }

    fn url(&self) -> &str {
        &self.url
    }
}

/// The server binds its socket last, so a connection that completes means it
/// is answering.
fn wait_until_listening() -> Result<()> {
    let address = SocketAddr::from(([127, 0, 0, 1], Consts::TEST_SERVER_PORT));
    let deadline = Instant::now() + Consts::TEST_SERVER_READY;
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&address, Consts::TEST_SERVER_POLL).is_ok() {
            return Ok(());
        }
        thread::sleep(Consts::TEST_SERVER_POLL);
    }
    bail!(
        "the hermetic test server did not listen on {address} within {}s",
        Consts::TEST_SERVER_READY.as_secs()
    )
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Safari goes through the repository's wasm recipe for the same reason the
/// other browsers do: the target rebuilds std for shared memory, and only the
/// pinned nightly honours that.
pub(crate) fn safari(process: &Process) -> Result<()> {
    process.require_os("macos", "Safari WASM")?;
    process.require_tools(&["cargo", "just"])?;
    process.run(
        "just",
        &["platform", "wasm", "test", "safari"],
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_and_trusted_pipelines_run_the_explicit_flash_and_no_block_gate() {
        let expected = [
            "test",
            "run",
            "--flash=on",
            "--no-block=on",
            "--profile",
            "ci",
        ];

        for kind in [
            PipelineKind::MergeRequest,
            PipelineKind::Branch,
            PipelineKind::Main,
            PipelineKind::Platforms,
            PipelineKind::Nightly,
            PipelineKind::Release,
        ] {
            let (args, _) = test_command(kind).unwrap();
            assert_eq!(args, expected);
        }
    }

    #[test]
    fn platform_runs_use_the_default_branch_apple_lint_gate() {
        let (args, label) = lint_command(PipelineKind::Platforms);

        assert_eq!(args, ["lint", "full"]);
        assert_eq!(label, "full lint gate");
    }

    #[test]
    fn quarantine_keeps_its_plain_profile_probe() {
        let (args, _) = test_command(PipelineKind::Quarantine).unwrap();

        assert_eq!(args, ["test", "run", "--profile", "ci"]);
    }

    #[test]
    fn weekly_pipeline_does_not_claim_to_run_the_apple_suite() {
        let error = test_command(PipelineKind::Weekly).unwrap_err();

        assert_eq!(
            error.to_string(),
            "weekly pipelines do not run the Apple suite"
        );
    }
}
