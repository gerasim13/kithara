use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use kithara_devtools::Ctx;

use super::{
    bridge::BridgeArgs, host::HostArgs, image::ImageArgs, linux::LinuxArgs, run::RunArgs,
    verdict::VerdictArgs,
};

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
    /// Provision and maintain a Linux CI machine and its runners.
    Linux(LinuxArgs),
    /// Execute one repository CI lane.
    Run(RunArgs),
    /// Record what the default branch fails, and hold a run that fails more.
    Verdict(VerdictArgs),
}

pub(crate) const fn is_standalone(args: &CiArgs) -> bool {
    matches!(
        args.command,
        CiCommand::Bridge(_)
            | CiCommand::Host(_)
            | CiCommand::Image(_)
            | CiCommand::Linux(_)
            | CiCommand::Verdict(_)
    )
}

pub(crate) fn run_standalone(args: &CiArgs) -> Result<()> {
    match &args.command {
        CiCommand::Bridge(args) => super::bridge::run(args),
        CiCommand::Host(args) => super::host::run(args),
        CiCommand::Image(args) => super::image::run(args),
        CiCommand::Linux(args) => super::linux::run(args),
        CiCommand::Verdict(args) => super::verdict::run(args),
        CiCommand::Run(_) => bail!("repository CI lanes require a workspace"),
    }
}

pub(crate) fn run(args: &CiArgs, ctx: &Ctx) -> Result<()> {
    match &args.command {
        CiCommand::Bridge(_)
        | CiCommand::Host(_)
        | CiCommand::Image(_)
        | CiCommand::Linux(_)
        | CiCommand::Verdict(_) => run_standalone(args),
        CiCommand::Run(args) => super::run::run(args, ctx),
    }
}
