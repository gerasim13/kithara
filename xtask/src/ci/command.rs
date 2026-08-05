use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use kithara_devtools::Ctx;

use super::{bridge::BridgeArgs, host::HostArgs, image::ImageArgs, run::RunArgs};

#[derive(Debug, Args)]
pub(crate) struct CiArgs {
    #[command(subcommand)]
    command: CiCommand,
}

#[derive(Debug, Subcommand)]
enum CiCommand {
    /// Reconcile the public GitHub and private `GitLab` repositories.
    Bridge(BridgeArgs),
    /// Provision and maintain the dedicated CI host.
    Host(HostArgs),
    /// Build the pinned container images a CI machine runs jobs in.
    Image(ImageArgs),
    /// Execute one repository CI lane.
    Run(RunArgs),
}

pub(crate) fn is_standalone(args: &CiArgs) -> bool {
    matches!(
        args.command,
        CiCommand::Bridge(_) | CiCommand::Host(_) | CiCommand::Image(_)
    )
}

pub(crate) fn run_standalone(args: &CiArgs) -> Result<()> {
    match &args.command {
        CiCommand::Bridge(args) => super::bridge::run(args),
        CiCommand::Host(args) => super::host::run(args),
        CiCommand::Image(args) => super::image::run(args),
        CiCommand::Run(_) => bail!("repository CI lanes require a workspace"),
    }
}

pub(crate) fn run(args: &CiArgs, ctx: &Ctx) -> Result<()> {
    match &args.command {
        CiCommand::Bridge(_) | CiCommand::Host(_) | CiCommand::Image(_) => run_standalone(args),
        CiCommand::Run(args) => super::run::run(args, ctx),
    }
}
