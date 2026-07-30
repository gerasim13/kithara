use std::env;

use anyhow::{Context, Result, bail};
use clap::{Args, ValueEnum};
use kithara_devtools::Ctx;
use tracing::info;

use super::{config::CiConfig, environment::CiEnvironment, process::Process};
use crate::config::KitharaExt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum Lane {
    Apple,
    Linux,
    Android,
    Web,
    Windows,
    Deep,
    Weekly,
    ReleaseApple,
    ReleaseAndroid,
    ReleasePublish,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum PipelineKind {
    MergeRequest,
    Quarantine,
    Main,
    Nightly,
    Weekly,
    Release,
}

#[derive(Debug, Args)]
pub(crate) struct RunArgs {
    /// CI lane to execute.
    lane: Lane,
    /// Pipeline policy used by lanes with fast and full variants.
    #[arg(
        long,
        env = "KITHARA_PIPELINE_KIND",
        value_enum,
        default_value = "merge-request"
    )]
    kind: PipelineKind,
}

pub(crate) fn run(args: &RunArgs, ctx: &Ctx) -> Result<()> {
    let ext = KitharaExt::from_ctx(ctx)?;
    ext.ci.validate()?;
    let ci_config = CiConfig::load(&ctx.root.join(&ext.ci.host_config))?;
    let environment = CiEnvironment::prepare(ctx, &ci_config, args.lane)?;
    info!(
        lane = ?args.lane,
        kind = ?args.kind,
        cache_root = %environment.cache_root.display(),
        "CI environment prepared"
    );
    let swiftpm_cache = environment.swiftpm_cache.clone();
    let temp = environment.temp.clone();
    let process = Process::new(&ctx.root, environment.vars());

    let result = match args.lane {
        Lane::Apple => run_apple(&process, &ci_config, args.kind, &swiftpm_cache),
        Lane::Linux => run_linux(&process, args.kind),
        Lane::Android => run_android(&process, &ci_config),
        Lane::Web => run_web(&process),
        Lane::Windows => run_windows(&process),
        Lane::Deep => run_deep(&process),
        Lane::Weekly => run_weekly(&process, args.kind),
        Lane::ReleaseApple => super::release::build_apple(&process, ctx, &ext, &temp),
        Lane::ReleaseAndroid => super::release::build_android(&process, ctx, &ext),
        Lane::ReleasePublish => super::release::publish(ctx, &ext),
    };
    process.best_effort("sccache", &["--show-stats"], "sccache statistics");
    result
}

fn run_apple(
    process: &Process,
    config: &CiConfig,
    kind: PipelineKind,
    swiftpm_cache: &std::path::Path,
) -> Result<()> {
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
    if actual != config.expected_xcode_version {
        bail!(
            "Xcode {} is required, found {actual}",
            config.expected_xcode_version
        );
    }

    process.run("just", &["lint", "full"], "full lint gate")?;
    let mut check = process.command("just");
    check
        .env("RUSTUP_TOOLCHAIN", &config.msrv_toolchain)
        .args(["check", "workspace"]);
    process.run_command(&mut check, "MSRV workspace check")?;

    match kind {
        PipelineKind::MergeRequest | PipelineKind::Quarantine => {
            process.run(
                "just",
                &["test", "run", "--profile", "ci"],
                "Apple merge-request tests",
            )?;
        }
        PipelineKind::Main | PipelineKind::Nightly | PipelineKind::Release => {
            process.run(
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
            )?;
            let mut flash_off = process.command("just");
            flash_off.env("CARGO_TARGET_DIR", "target-flash-off").args([
                "test",
                "run",
                "--flash=off",
                "--profile",
                "ci",
            ]);
            process.run_command(&mut flash_off, "Apple flash-off gate")?;
        }
        PipelineKind::Weekly => bail!("weekly pipelines do not use the Apple lane"),
    }

    process.run(
        "just",
        &["platform", "apple", "xcframework", "--profile", "debug"],
        "Apple XCFramework",
    )?;
    let mut swift_test = process.command("swift");
    swift_test
        .env("KITHARA_LOCAL_DEV", "1")
        .arg("test")
        .arg("--cache-path")
        .arg(swiftpm_cache);
    process.run_command(&mut swift_test, "Swift package tests")?;
    let mut ios = process.command("just");
    ios.env("KITHARA_LOCAL_DEV", "1")
        .args(["platform", "apple", "ios"]);
    process.run_command(&mut ios, "iOS Simulator build")?;

    if kind == PipelineKind::Nightly {
        process.require_tools(&["wasm-pack"])?;
        process.run(
            "wasm-pack",
            &["test", "tests", "--headless", "--safari"],
            "Safari WASM tests",
        )?;
    }
    Ok(())
}

fn run_linux(process: &Process, kind: PipelineKind) -> Result<()> {
    require_os("linux", "Linux")?;
    process.require_tools(&["cargo", "gitleaks", "just", "sccache"])?;
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
    )?;
    process.run("just", &["check", "workspace"], "Linux workspace check")?;
    process.run("just", &["platform", "wasm"], "WASM portability check")?;

    match kind {
        PipelineKind::MergeRequest | PipelineKind::Quarantine => {}
        PipelineKind::Main | PipelineKind::Nightly => {
            process.run("just", &["test", "run", "--profile", "ci"], "Linux tests")?;
        }
        PipelineKind::Release | PipelineKind::Weekly => {
            bail!("{kind:?} pipelines do not use the Linux portability lane");
        }
    }
    if kind == PipelineKind::Nightly {
        let mut coverage = process.command("just");
        coverage
            .env("COVERAGE_OUTPUT_DIR", "coverage")
            .env("COVERAGE_MIN", "80")
            .args(["test", "coverage"]);
        process.run_command(&mut coverage, "Linux coverage")?;
    }
    Ok(())
}

fn run_android(process: &Process, config: &CiConfig) -> Result<()> {
    require_os("macos", "Android emulator")?;
    process.require_tools(&["adb", "cargo", "emulator", "java", "just", "sccache"])?;
    process.run(
        "just",
        &["platform", "android", "build"],
        "Android native build",
    )?;
    process.run(
        "just",
        &[
            "platform",
            "android",
            "run",
            "--avd",
            &config.android_avd,
            "--skip-build",
        ],
        "Android emulator launch",
    )?;

    let mut gradle = process.command(if cfg!(windows) {
        "android/gradlew.bat"
    } else {
        "android/gradlew"
    });
    gradle.args([
        ":lib:connectedDebugAndroidTest",
        "-x",
        "generateKitharaFfi",
        "--no-daemon",
    ]);
    let tests = process.run_command(&mut gradle, "Android connected tests");
    let cleanup = process.run("adb", &["emu", "kill"], "Android emulator shutdown");
    match (tests, cleanup) {
        (Err(error), _) => Err(error),
        (Ok(()), cleanup) => cleanup,
    }
}

fn run_web(process: &Process) -> Result<()> {
    require_os("linux", "Web browser")?;
    process.require_tools(&[
        "chromium",
        "firefox",
        "geckodriver",
        "chromedriver",
        "just",
        "wasm-pack",
    ])?;
    process.run(
        "wasm-pack",
        &["test", "tests", "--headless", "--chrome", "--firefox"],
        "Chromium and Firefox WASM tests",
    )?;
    process.run(
        "just",
        &["platform", "wasm", "size-check"],
        "WASM size check",
    )
}

fn run_windows(process: &Process) -> Result<()> {
    require_os("windows", "Windows")?;
    process.require_tools(&["cargo", "rustup", "sccache"])?;
    for target in ["aarch64-pc-windows-msvc", "x86_64-pc-windows-msvc"] {
        process.run(
            "rustup",
            &["target", "add", target],
            "install Windows target",
        )?;
        process.run(
            "cargo",
            &["xtask", "test", "--profile", "ci", "--target", target],
            "Windows tests",
        )?;
    }
    Ok(())
}

fn run_deep(process: &Process) -> Result<()> {
    require_os("macos", "deep Apple")?;
    process.require_tools(&["cargo", "just"])?;
    for command in [
        ["test", "rtsan"].as_slice(),
        ["test", "rtsan-file"].as_slice(),
        ["test", "rtsan-hls"].as_slice(),
        ["perf", "memory"].as_slice(),
        ["perf"].as_slice(),
        ["perf", "bench", "ci"].as_slice(),
    ] {
        process.run("just", command, "deep quality stage")?;
    }
    Ok(())
}

fn run_weekly(process: &Process, kind: PipelineKind) -> Result<()> {
    require_os("linux", "weekly")?;
    if kind != PipelineKind::Weekly {
        bail!("weekly lane requires --kind weekly");
    }
    process.require_tools(&["cargo", "just"])?;
    for command in [
        ["deps", "deny"].as_slice(),
        ["deps", "machete"].as_slice(),
        ["deps", "shear"].as_slice(),
        ["deps", "hack"].as_slice(),
        ["deps", "semver"].as_slice(),
    ] {
        process.run("just", command, "weekly dependency gate")?;
    }
    process.run(
        "just",
        &[
            "test",
            "mutants",
            "run-all",
            "--output",
            "target/mutants-ci",
            "--jobs",
            "1",
        ],
        "focused mutation suites",
    )
}

fn require_os(expected: &str, label: &str) -> Result<()> {
    if env::consts::OS != expected {
        bail!(
            "{label} lane requires {expected}, current platform is {}",
            env::consts::OS
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::Cli;

    #[test]
    fn ci_lane_is_typed_and_uses_modular_names() {
        assert!(Cli::try_parse_from(["xtask", "ci", "run", "apple"]).is_ok());
        assert!(
            Cli::try_parse_from(["xtask", "ci", "run", "linux", "--kind", "quarantine"]).is_ok()
        );
        assert!(Cli::try_parse_from(["xtask", "ci", "run", "unknown"]).is_err());
    }
}
