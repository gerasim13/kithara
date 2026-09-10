use std::{
    collections::BTreeMap,
    env,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};

use super::config::{CiPins, PINS_PATH};
use crate::ci::host::read_secret;

mod provision;
mod verify;

const CLIENT_KEYS: [&str; 7] = [
    "SCCACHE_BUCKET",
    "SCCACHE_ENDPOINT",
    "SCCACHE_REGION",
    "SCCACHE_S3_USE_SSL",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_EC2_METADATA_DISABLED",
];

/// Read the restricted environment a cache client may inherit.
pub(crate) fn client_environment(path: &Path) -> Result<BTreeMap<String, String>> {
    let mut environment = BTreeMap::new();
    for line in read_secret(path)?.lines() {
        let (key, value) = line
            .split_once('=')
            .context("invalid cache environment entry")?;
        ensure!(
            CLIENT_KEYS.contains(&key),
            "unexpected cache environment key"
        );
        ensure!(!value.is_empty(), "empty cache environment value");
        ensure!(
            !value
                .chars()
                .any(|character| character.is_control() || matches!(character, '"' | '\\')),
            "unsafe cache environment value"
        );
        ensure!(
            environment
                .insert(key.to_owned(), value.to_owned())
                .is_none(),
            "duplicate cache environment key"
        );
    }
    ensure!(
        environment.len() == CLIENT_KEYS.len(),
        "incomplete cache environment"
    );
    Ok(environment)
}

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
