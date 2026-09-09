use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};

use super::config::{CiPins, PINS_PATH};

mod provision;
mod verify;

#[derive(Debug, Args)]
pub(crate) struct CacheArgs {
    #[command(subcommand)]
    command: CacheCommand,
}

#[derive(Debug, Subcommand)]
enum CacheCommand {
    /// Operate the shared compiler cache through Docker Compose.
    Compose {
        env_file: PathBuf,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<String>,
    },
    /// Create persistent administrator credentials inside the Compose volume.
    Credentials,
    /// Initialize isolated buckets and client credentials inside Compose.
    Initialize,
    /// Verify cache reuse between two independent compiler daemons.
    Verify { env_file: PathBuf },
}

pub(crate) fn run(args: &CacheArgs) -> Result<()> {
    match &args.command {
        CacheCommand::Compose {
            env_file,
            arguments,
        } => {
            let pins = CiPins::load(Path::new(PINS_PATH))?;
            let status = Command::new("docker")
                .args(["compose", "--env-file"])
                .arg(env_file)
                .args(["-f", "docker/ci-cache.compose.yml"])
                .args(arguments)
                .env("KITHARA_CACHE_IMAGE", &pins.sccache_s3_image)
                .env("KITHARA_RUST_VERSION", &pins.stable_toolchain)
                .env("KITHARA_RUST_DIGEST", &pins.linux_base_digest)
                .status()
                .context("run cache Compose")?;
            ensure!(status.success(), "cache Compose exited with {status}");
            Ok(())
        }
        CacheCommand::Credentials => provision::credentials(),
        CacheCommand::Initialize => provision::initialize(),
        CacheCommand::Verify { env_file } => verify::run(env_file),
    }
}

fn required(name: &str) -> Result<String> {
    let value = env::var(name).with_context(|| format!("{name} must be configured"))?;
    ensure!(!value.trim().is_empty(), "{name} must not be empty");
    Ok(value)
}
