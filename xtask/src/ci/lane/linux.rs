use anyhow::Result;

use crate::ci::process::{Process, require_os};

fn preflight(process: &Process) -> Result<()> {
    require_os("linux", "Linux")?;
    process.require_tools(&["cargo", "just", "sccache"])
}

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
    process.run("just", &["test", "run", "--profile", "ci"], "Linux tests")
}

pub(crate) fn coverage(process: &Process) -> Result<()> {
    preflight(process)?;
    let mut command = process.command("just");
    command
        .env("COVERAGE_OUTPUT_DIR", "coverage")
        .env("COVERAGE_MIN", "80")
        .args(["test", "coverage"]);
    process.run_command(&mut command, "Linux coverage")
}
