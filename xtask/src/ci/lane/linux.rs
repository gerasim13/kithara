use anyhow::Result;

use crate::ci::process::{Process, require_os};

fn preflight(process: &Process) -> Result<()> {
    require_os("linux", "Linux")?;
    process.require_tools(&["cargo", "just", "sccache"])
}

/// The scan reports where it found something, not merely how many. Without
/// the findings it announced "leaks found: 5" and stopped, which is a number
/// nobody can act on. Values stay redacted, so the log and the report name the
/// rule, file, and commit without repeating the secret.
pub(crate) fn secrets(process: &Process) -> Result<()> {
    require_os("linux", "Linux")?;
    process.require_tools(&["gitleaks"])?;
    process.run(
        "gitleaks",
        &[
            "git",
            ".",
            "--config",
            ".gitleaks.toml",
            "--log-opts=--all",
            "--no-banner",
            "--redact=100",
            "--verbose",
            "--report-format",
            "json",
            "--report-path",
            "gitleaks-report.json",
        ],
        "repository secret scan",
    )
}

pub(crate) fn check(process: &Process) -> Result<()> {
    preflight(process)?;
    process.run("just", &["check", "workspace"], "Linux workspace check")
}

pub(crate) fn wasm(process: &Process) -> Result<()> {
    preflight(process)?;
    process.run("just", &["platform", "wasm"], "WASM portability check")
}

pub(crate) fn test(process: &Process) -> Result<()> {
    preflight(process)?;
    let mut command = process.command("just");
    command
        // The container has five CPUs and under eight gigabytes, so Cargo runs
        // five compilations at once and the peak lands past the cgroup limit.
        // The kernel logged `Killed process (rustc) anon-rss:1727320kB` twice
        // in one second, and Cargo reported only "could not compile" with no
        // diagnostic, because the compiler never reached one. The coverage
        // lane keeps to two for the same reason.
        .env("CARGO_BUILD_JOBS", "2")
        .args(["test", "run", "--profile", "ci"]);
    process.run_command(&mut command, "Linux tests")
}

/// One lane configured in `.config/xtask.toml`. Two build jobs for the same
/// reason `test` keeps to two.
pub(crate) fn configured(process: &Process, lane: &str) -> Result<()> {
    preflight(process)?;
    let mut command = process.command("just");
    command
        .env("CARGO_BUILD_JOBS", "2")
        .args(["test", "run", &format!("--lane={lane}")]);
    process.run_command(&mut command, &format!("Linux {lane} lane"))
}

/// A selenium lane per browser: the lane name (`selenium-<browser>`) is the
/// only place the browser is decided. The lane's own configuration carries it
/// into the harness, so a run started here and a run started by hand exercise
/// the same browser.
pub(crate) fn selenium(process: &Process, browser: &str) -> Result<()> {
    preflight(process)?;
    let lane = format!("selenium-{browser}");
    let mut command = process.command("just");
    command
        .env("CARGO_BUILD_JOBS", "2")
        .args(["test", "run", &format!("--lane={lane}")]);
    process.run_command(&mut command, &format!("Linux {lane} lane"))
}

pub(crate) fn coverage(process: &Process) -> Result<()> {
    preflight(process)?;
    let mut command = process.command("just");
    command
        .env("COVERAGE_OUTPUT_DIR", "coverage")
        // Instrumented test binaries are large and the workspace has twenty of
        // them, so Cargo reaches the link step with several `ld` processes at
        // once. In a five-gigabyte VM the kernel picks one and kills it, and
        // the only trace it leaves is `ld terminated with signal 9`. Two units
        // at a time keeps the peak inside the VM; the lane runs nightly and can
        // afford the wall-clock.
        .env("CARGO_BUILD_JOBS", "2")
        .args(["test", "coverage"]);
    process.run_command(&mut command, "Linux coverage")
}
