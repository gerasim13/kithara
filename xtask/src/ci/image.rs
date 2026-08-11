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
    /// Build the Android emulator image on top of the toolchain image.
    Android,
    /// Build the runner image an Android job starts in.
    AndroidRunner,
}

impl ImageCommand {
    pub(crate) const fn dockerfile(self) -> &'static str {
        match self {
            Self::Toolchain => "docker/ci.Dockerfile",
            // One recipe serves both runners; they differ by the image they
            // are layered on, which is a build argument rather than a file.
            Self::Runner | Self::AndroidRunner => "docker/ci-runner.Dockerfile",
            Self::Android => "docker/ci-android.Dockerfile",
        }
    }

    fn tag(self, pins: &CiPins) -> &str {
        match self {
            Self::Toolchain => &pins.linux_image,
            Self::Runner => &pins.linux_runner_image,
            Self::Android => &pins.linux_android_image,
            Self::AndroidRunner => &pins.linux_android_runner_image,
        }
    }

    pub(crate) fn build_args(
        self,
        pins: &CiPins,
        root: &std::path::Path,
    ) -> Result<Vec<(&'static str, String)>> {
        match self {
            Self::Toolchain => linux_build_args(pins),
            Self::Runner => Ok(runner_build_args(pins, &pins.linux_image)),
            Self::Android => android_build_args(pins, root),
            Self::AndroidRunner => Ok(runner_build_args(pins, &pins.linux_android_image)),
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
        &args.command.build_args(&pins, &root)?,
    )
}

/// Build one image with no context at all. Every Dockerfile here downloads what
/// it needs and copies nothing, so the recipe arrives on standard input and the
/// working tree is never sent to the daemon.
fn build(
    process: &Process,
    dockerfile: &str,
    tag: &str,
    arguments: &[(&'static str, String)],
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
pub(crate) fn linux_build_args(pins: &CiPins) -> Result<Vec<(&'static str, String)>> {
    let mut args = vec![
        ("RUST_VERSION", pins.stable_toolchain.clone()),
        ("RUST_BASE_DIGEST", pins.linux_base_digest.clone()),
        ("MSRV_TOOLCHAIN", pins.msrv_toolchain.clone()),
        ("NIGHTLY_TOOLCHAIN", pins.nightly_toolchain.clone()),
        ("CMAKE_VERSION", pins.cmake_version.clone()),
        ("CMAKE_AMD64_SHA256", pins.cmake_linux_amd64_sha256.clone()),
        ("CMAKE_ARM64_SHA256", pins.cmake_linux_arm64_sha256.clone()),
        ("GECKODRIVER_VERSION", pins.geckodriver_version.clone()),
        (
            "GECKODRIVER_AMD64_SHA256",
            pins.geckodriver_linux_amd64_sha256.clone(),
        ),
        (
            "GECKODRIVER_ARM64_SHA256",
            pins.geckodriver_linux_arm64_sha256.clone(),
        ),
        ("GITLEAKS_VERSION", pins.gitleaks_version.clone()),
        (
            "GITLEAKS_AMD64_SHA256",
            pins.gitleaks_linux_amd64_sha256.clone(),
        ),
        (
            "GITLEAKS_ARM64_SHA256",
            pins.gitleaks_linux_arm64_sha256.clone(),
        ),
    ];
    for (name, tool) in [
        ("AST_GREP_VERSION", "ast-grep"),
        ("CARGO_CRAP_VERSION", "cargo-crap"),
        ("CARGO_DENY_VERSION", "cargo-deny"),
        ("CARGO_GEIGER_VERSION", "cargo-geiger"),
        ("CARGO_HACK_VERSION", "cargo-hack"),
        ("CARGO_LLVM_COV_VERSION", "cargo-llvm-cov"),
        ("CARGO_MACHETE_VERSION", "cargo-machete"),
        ("CARGO_MODULES_VERSION", "cargo-modules"),
        ("CARGO_MUTANTS_VERSION", "cargo-mutants"),
        ("CARGO_NEXTEST_VERSION", "cargo-nextest"),
        ("CARGO_SEMVER_CHECKS_VERSION", "cargo-semver-checks"),
        ("CARGO_SHEAR_VERSION", "cargo-shear"),
        ("CARGO_SORT_VERSION", "cargo-sort"),
        (
            "CARGO_WORKSPACE_UNUSED_PUB_VERSION",
            "cargo-workspace-unused-pub",
        ),
        ("JUST_VERSION", "just"),
        ("MD_FORMATTER_VERSION", "md-formatter"),
        ("SCCACHE_VERSION", "sccache"),
        ("SIMILARITY_RS_VERSION", "similarity-rs"),
        ("TAPLO_CLI_VERSION", "taplo-cli"),
        ("TIDY_JSON_VERSION", "tidy-json"),
        ("TRUNK_VERSION", "trunk"),
        ("TYPOS_CLI_VERSION", "typos-cli"),
        ("WASM_BINDGEN_CLI_VERSION", "wasm-bindgen-cli"),
        ("WASM_PACK_VERSION", "wasm-pack"),
        ("WASM_SLIM_VERSION", "wasm-slim"),
    ] {
        args.push((name, pins.cargo_tool_version(tool)?.to_owned()));
    }
    Ok(args)
}

pub(crate) fn runner_build_args(pins: &CiPins, base: &str) -> Vec<(&'static str, String)> {
    vec![
        ("CI_IMAGE", base.to_owned()),
        (
            "ACTIONS_RUNNER_VERSION",
            pins.actions_runner_version.clone(),
        ),
        (
            "ACTIONS_RUNNER_AMD64_SHA256",
            pins.actions_runner_linux_amd64_sha256.clone(),
        ),
        (
            "ACTIONS_RUNNER_ARM64_SHA256",
            pins.actions_runner_linux_arm64_sha256.clone(),
        ),
    ]
}

/// The emulator this image carries is the one the workspace boots, so its name
/// is read from the workspace rather than repeated here. Two names for one
/// device is a machine that builds an emulator nothing asks for.
fn android_build_args(
    pins: &CiPins,
    root: &std::path::Path,
) -> Result<Vec<(&'static str, String)>> {
    let android = crate::config::KitharaExt::load(root)?.android;
    Ok(vec![
        ("CI_IMAGE", pins.linux_image.clone()),
        ("ANDROID_AVD", android.default_avd),
        (
            "ANDROID_BUILD_TOOLS_VERSION",
            pins.android_build_tools_version.clone(),
        ),
        (
            "ANDROID_CMDLINE_TOOLS_LINUX_SHA256",
            pins.android_commandline_tools_linux_sha256.clone(),
        ),
        (
            "ANDROID_CMDLINE_TOOLS_VERSION",
            pins.android_commandline_tools_version.clone(),
        ),
        ("ANDROID_NDK_VERSION", pins.android_ndk_version.clone()),
        (
            "ANDROID_PLATFORM_VERSION",
            pins.android_platform_version.to_string(),
        ),
        (
            "CARGO_NDK_VERSION",
            pins.cargo_tool_version("cargo-ndk")?.to_owned(),
        ),
    ])
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

    fn configured_arguments(arguments: Vec<(&'static str, String)>) -> BTreeSet<String> {
        arguments
            .into_iter()
            .map(|(name, _)| name.to_owned())
            .collect()
    }

    #[test]
    fn dockerfile_versions_are_owned_by_typed_config() {
        let pins = &fixture().pins;
        for image in [
            ImageCommand::Toolchain,
            ImageCommand::Runner,
            ImageCommand::Android,
            ImageCommand::AndroidRunner,
        ] {
            let dockerfile = image.dockerfile();
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("the crate sits inside the workspace");
            let configured = configured_arguments(image.build_args(pins, root).unwrap());
            assert_eq!(declared_arguments(dockerfile), configured, "{dockerfile}");
        }
    }
}
