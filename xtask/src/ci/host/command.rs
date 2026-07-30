use std::{collections::BTreeMap, path::PathBuf};

use anyhow::Result;
use clap::{Args, Subcommand};

use super::{
    runners::RunnerManager, services::ServiceInstaller, storage::HostStorage, system::SystemSetup,
    toolchain::ToolchainInstaller,
};
use crate::ci::{
    config::{CiConfig, PINS_PATH},
    process::Process,
};

#[derive(Debug, Args)]
pub(crate) struct HostArgs {
    /// Machine profile of this CI host, provisioned outside the repository.
    #[arg(long, env = "KITHARA_CI_HOST_CONFIG")]
    config: PathBuf,
    /// Reviewed build pins tracked in the repository.
    #[arg(long, env = "KITHARA_CI_PINS", default_value = PINS_PATH)]
    pins: PathBuf,
    #[command(subcommand)]
    command: HostCommand,
}

#[derive(Debug, Subcommand)]
enum HostCommand {
    /// Create and validate the bounded APFS volume and dedicated CI account.
    Bootstrap,
    /// Configure Xcode and install the root-owned CI service binary.
    Finish,
    /// Install host packages through the existing Homebrew installation.
    InstallHostTools,
    /// Install pinned Rust, Android, and Cilicon tools for the CI account.
    InstallUserTools,
    /// Install the current Rust executable and launchd service definitions.
    InstallServices,
    /// Validate credentials and activate the repository bridge daemon.
    ActivateBridge,
    /// Configure `GitLab` runners from local token files.
    ConfigureRunners,
    /// Load or reload the CI user's launchd services.
    Activate,
    /// Prepare a disposable Cilicon guest before `GitLab` Runner starts.
    GuestPrepare,
    /// Build and pin the Linux runner image.
    BuildLinuxImage {
        /// Dockerfile used to build the pinned Linux image.
        dockerfile: PathBuf,
    },
    /// Verify the pinned Linux runner image and its required tools.
    SmokeLinux,
    /// Boot the Android emulator and verify it reaches a ready state.
    SmokeAndroid,
    /// Verify the pinned Cilicon image and start Cilicon.
    StartCilicon,
    /// Reject a job before it can fill or damage the CI volume.
    Preflight,
    /// Remove expired job state and bounded cache entries.
    Cleanup,
    /// Emit host storage and runner health as JSON.
    Health,
}

pub(crate) fn run(args: &HostArgs) -> Result<()> {
    let config = CiConfig::load(&args.config, &args.pins)?;
    config.validate_macos_layout()?;
    let root = std::env::current_dir()?;
    let process = Process::new(&root, BTreeMap::new());
    match &args.command {
        HostCommand::Bootstrap => SystemSetup::new(&config, &process).bootstrap(),
        HostCommand::Finish => {
            SystemSetup::new(&config, &process).finish()?;
            ServiceInstaller::new(&config, &process).install(&args.config, &args.pins)
        }
        HostCommand::InstallHostTools => {
            ToolchainInstaller::new(&config, &process).install_host_tools()
        }
        HostCommand::InstallUserTools => {
            ToolchainInstaller::new(&config, &process).install_user_tools()
        }
        HostCommand::InstallServices => {
            ServiceInstaller::new(&config, &process).install(&args.config, &args.pins)
        }
        HostCommand::ActivateBridge => ServiceInstaller::new(&config, &process).activate_bridge(),
        HostCommand::ConfigureRunners => RunnerManager::new(&config, &process).configure(),
        HostCommand::Activate => RunnerManager::new(&config, &process).activate(),
        HostCommand::GuestPrepare => RunnerManager::new(&config, &process).prepare_guest(),
        HostCommand::BuildLinuxImage { dockerfile } => {
            RunnerManager::new(&config, &process).build_linux_image(dockerfile)
        }
        HostCommand::SmokeLinux => RunnerManager::new(&config, &process).smoke_linux(),
        HostCommand::SmokeAndroid => RunnerManager::new(&config, &process).smoke_android(),
        HostCommand::StartCilicon => RunnerManager::new(&config, &process).start_cilicon(),
        HostCommand::Preflight => HostStorage::new(&config, &process)?.preflight(),
        HostCommand::Cleanup => HostStorage::new(&config, &process)?.cleanup(),
        HostCommand::Health => HostStorage::new(&config, &process)?.health(),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::Cli;

    fn parse(command: &[&str]) -> Result<Cli, clap::Error> {
        let mut argv = vec![
            "xtask",
            "ci",
            "host",
            "--config",
            "/etc/kithara-ci/host.json",
        ];
        argv.extend_from_slice(command);
        Cli::try_parse_from(argv)
    }

    #[test]
    fn host_maintenance_commands_are_typed() {
        for command in [
            ["bootstrap"].as_slice(),
            ["finish"].as_slice(),
            ["install-host-tools"].as_slice(),
            ["install-user-tools"].as_slice(),
            ["install-services"].as_slice(),
            ["activate-bridge"].as_slice(),
            ["configure-runners"].as_slice(),
            ["activate"].as_slice(),
            ["guest-prepare"].as_slice(),
            ["build-linux-image", "docker/ci.Dockerfile"].as_slice(),
            ["smoke-linux"].as_slice(),
            ["smoke-android"].as_slice(),
            ["start-cilicon"].as_slice(),
            ["preflight"].as_slice(),
            ["cleanup"].as_slice(),
            ["health"].as_slice(),
        ] {
            assert!(parse(command).is_ok(), "{command:?} must parse");
        }
        assert!(parse(&["rm"]).is_err());
    }
}
