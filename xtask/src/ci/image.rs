use std::{collections::BTreeMap, fs, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use tracing::info;

use super::{
    config::{CiPins, PINS_PATH},
    process::Process,
};

#[derive(Debug, Args)]
pub(crate) struct ImageArgs {
    /// Reviewed build pins tracked in the repository.
    #[arg(long, env = "KITHARA_CI_PINS", default_value = PINS_PATH)]
    pins: PathBuf,
    #[command(subcommand)]
    command: ImageCommand,
}

#[derive(Clone, Copy, Debug, Subcommand)]
pub(crate) enum ImageCommand {
    /// Build the pinned toolchain image for this machine's architecture.
    Toolchain,
    /// Build the GitHub Actions runner image on top of the toolchain image.
    Runner,
}

impl ImageCommand {
    pub(crate) fn dockerfile(self) -> &'static str {
        match self {
            Self::Toolchain => "docker/ci.Dockerfile",
            Self::Runner => "docker/ci-runner.Dockerfile",
        }
    }

    fn tag(self, pins: &CiPins) -> &str {
        match self {
            Self::Toolchain => &pins.linux_image,
            Self::Runner => &pins.linux_runner_image,
        }
    }

    pub(crate) fn build_args(self, pins: &CiPins) -> Result<Vec<(&'static str, &str)>> {
        match self {
            Self::Toolchain => linux_build_args(pins),
            Self::Runner => Ok(runner_build_args(pins)),
        }
    }
}

pub(crate) fn run(args: &ImageArgs) -> Result<()> {
    let pins = CiPins::load(&args.pins)?;
    let root = std::env::current_dir()?;
    let process = Process::new(&root, BTreeMap::new());
    process.require_tools(&["docker"])?;
    build(
        &process,
        args.command.dockerfile(),
        args.command.tag(&pins),
        &args.command.build_args(&pins)?,
    )
}

/// Build one image with no context at all. Every Dockerfile here downloads what
/// it needs and copies nothing, so the recipe arrives on standard input and the
/// working tree is never sent to the daemon.
fn build(
    process: &Process,
    dockerfile: &str,
    tag: &str,
    arguments: &[(&'static str, &str)],
) -> Result<()> {
    let recipe =
        fs::File::open(dockerfile).with_context(|| format!("reading Dockerfile {dockerfile}"))?;
    let mut command = process.command("docker");
    command.args(["build", "--tag", tag]);
    for (name, value) in arguments {
        command.arg("--build-arg").arg(format!("{name}={value}"));
    }
    command.arg("-").stdin(recipe);
    process.run_command(&mut command, "build pinned CI image")?;
    info!(image = tag, dockerfile, "CI image built");
    Ok(())
}

/// Build arguments for the toolchain image. Each download that differs by
/// architecture carries a checksum per slice; the Dockerfile picks between them.
pub(crate) fn linux_build_args(pins: &CiPins) -> Result<Vec<(&'static str, &str)>> {
    let mut args = vec![
        ("RUST_VERSION", pins.stable_toolchain.as_str()),
        ("RUST_BASE_DIGEST", pins.linux_base_digest.as_str()),
        ("MSRV_TOOLCHAIN", pins.msrv_toolchain.as_str()),
        ("NIGHTLY_TOOLCHAIN", pins.nightly_toolchain.as_str()),
        ("CMAKE_VERSION", pins.cmake_version.as_str()),
        ("CMAKE_AMD64_SHA256", pins.cmake_linux_amd64_sha256.as_str()),
        ("CMAKE_ARM64_SHA256", pins.cmake_linux_arm64_sha256.as_str()),
        ("GECKODRIVER_VERSION", pins.geckodriver_version.as_str()),
        (
            "GECKODRIVER_AMD64_SHA256",
            pins.geckodriver_linux_amd64_sha256.as_str(),
        ),
        (
            "GECKODRIVER_ARM64_SHA256",
            pins.geckodriver_linux_arm64_sha256.as_str(),
        ),
        ("GITLEAKS_VERSION", pins.gitleaks_version.as_str()),
        (
            "GITLEAKS_AMD64_SHA256",
            pins.gitleaks_linux_amd64_sha256.as_str(),
        ),
        (
            "GITLEAKS_ARM64_SHA256",
            pins.gitleaks_linux_arm64_sha256.as_str(),
        ),
    ];
    for (name, tool) in [
        ("AST_GREP_VERSION", "ast-grep"),
        ("CARGO_DENY_VERSION", "cargo-deny"),
        ("CARGO_HACK_VERSION", "cargo-hack"),
        ("CARGO_LLVM_COV_VERSION", "cargo-llvm-cov"),
        ("CARGO_MACHETE_VERSION", "cargo-machete"),
        ("CARGO_MUTANTS_VERSION", "cargo-mutants"),
        ("CARGO_NEXTEST_VERSION", "cargo-nextest"),
        ("CARGO_SEMVER_CHECKS_VERSION", "cargo-semver-checks"),
        ("CARGO_SHEAR_VERSION", "cargo-shear"),
        ("CARGO_SORT_VERSION", "cargo-sort"),
        ("JUST_VERSION", "just"),
        ("MD_FORMATTER_VERSION", "md-formatter"),
        ("SCCACHE_VERSION", "sccache"),
        ("SIMILARITY_RS_VERSION", "similarity-rs"),
        ("TAPLO_CLI_VERSION", "taplo-cli"),
        ("TIDY_JSON_VERSION", "tidy-json"),
        ("TYPOS_CLI_VERSION", "typos-cli"),
        ("WASM_BINDGEN_CLI_VERSION", "wasm-bindgen-cli"),
        ("WASM_PACK_VERSION", "wasm-pack"),
        ("WASM_SLIM_VERSION", "wasm-slim"),
    ] {
        args.push((name, pins.cargo_tool_version(tool)?));
    }
    Ok(args)
}

pub(crate) fn runner_build_args(pins: &CiPins) -> Vec<(&'static str, &str)> {
    vec![
        ("CI_IMAGE", pins.linux_image.as_str()),
        (
            "ACTIONS_RUNNER_VERSION",
            pins.actions_runner_version.as_str(),
        ),
        (
            "ACTIONS_RUNNER_AMD64_SHA256",
            pins.actions_runner_linux_amd64_sha256.as_str(),
        ),
        (
            "ACTIONS_RUNNER_ARM64_SHA256",
            pins.actions_runner_linux_arm64_sha256.as_str(),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::ci::config::{fixture, workspace_root};

    fn declared_arguments(dockerfile: &str) -> BTreeSet<String> {
        let text = fs::read_to_string(workspace_root().join(dockerfile)).unwrap();
        text.lines()
            .filter_map(|line| line.strip_prefix("ARG "))
            .map(|argument| {
                assert!(
                    !argument.contains('='),
                    "Docker build argument must not own a default: {argument}"
                );
                argument.to_owned()
            })
            .collect()
    }

    fn configured_arguments(arguments: Vec<(&'static str, &str)>) -> BTreeSet<String> {
        arguments
            .into_iter()
            .map(|(name, _)| name.to_owned())
            .collect()
    }

    #[test]
    fn dockerfile_versions_are_owned_by_typed_config() {
        let pins = &fixture().pins;
        for image in [ImageCommand::Toolchain, ImageCommand::Runner] {
            let dockerfile = image.dockerfile();
            let configured = configured_arguments(image.build_args(pins).unwrap());
            assert_eq!(declared_arguments(dockerfile), configured, "{dockerfile}");
        }
    }
}
