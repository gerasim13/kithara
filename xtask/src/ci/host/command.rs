use anyhow::Result;
use clap::{Args, Subcommand};

use super::{linux::command::LinuxArgs, mac::command::MacArgs};

/// One CI machine, addressed by the platform it serves.
///
/// Both machines answer the same questions — which images they run, which
/// runners they register, which agents they keep alive — so they share a
/// command and differ in the platform that owns the answers.
#[derive(Debug, Args)]
pub(crate) struct HostArgs {
    #[command(subcommand)]
    platform: HostPlatform,
}

#[derive(Debug, Subcommand)]
enum HostPlatform {
    /// The macOS machine: its Apple lanes, its Linux container and its guests.
    Mac(MacArgs),
    /// A Linux machine and the runners it serves.
    Linux(LinuxArgs),
}

pub(crate) fn run(args: &HostArgs) -> Result<()> {
    match &args.platform {
        HostPlatform::Mac(args) => super::mac::run(args),
        HostPlatform::Linux(args) => super::linux::run(args),
    }
}
