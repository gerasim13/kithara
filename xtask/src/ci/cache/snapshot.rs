use std::{
    collections::BTreeMap,
    env,
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
};

use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};
use kithara_devtools::lease;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use tracing::info;

use super::current_client_environment;
use crate::ci::environment::CacheTrust;

struct Snapshot;

impl Snapshot {
    const SCHEMA: &str = "kithara-target-snapshot-v1";
    const PREFIX: &str = "target-snapshots";
    const TRUSTED_BUCKET: &str = "kithara-trusted";
}

#[derive(Debug, Args)]
pub(super) struct SnapshotArgs {
    #[command(subcommand)]
    command: SnapshotCommand,
}

#[derive(Debug, Subcommand)]
enum SnapshotCommand {
    /// Print the target snapshot key for this lane and toolchain.
    Fingerprint {
        #[arg(long)]
        lane: String,
        #[arg(long)]
        profile: String,
        #[arg(long, default_value = "host")]
        target: String,
    },
    /// Restore a trusted immutable snapshot into an empty private target directory.
    Restore {
        #[arg(long)]
        target: PathBuf,
        #[arg(long)]
        fingerprint: String,
    },
    /// Publish an immutable target snapshot from a trusted job.
    Publish {
        #[arg(long)]
        target: PathBuf,
        #[arg(long)]
        fingerprint: String,
    },
}

pub(super) fn run(args: &SnapshotArgs) -> Result<()> {
    match &args.command {
        SnapshotCommand::Fingerprint {
            lane,
            profile,
            target,
        } => {
            println!("{}", fingerprint(lane, profile, target, Path::new("."))?);
            Ok(())
        }
        SnapshotCommand::Restore {
            target,
            fingerprint,
        } => restore(target, fingerprint),
        SnapshotCommand::Publish {
            target,
            fingerprint,
        } => publish(target, fingerprint),
    }
}

pub(crate) fn restore_for_lane(key: &str, target: &Path, root: &Path) -> Result<String> {
    let fingerprint = fingerprint(key, "cargo", "host", root)?;
    restore(target, &fingerprint)?;
    Ok(fingerprint)
}

pub(crate) fn publish_for_lane(target: &Path, fingerprint: &str) -> Result<()> {
    publish(target, fingerprint)
}

fn fingerprint(lane: &str, profile: &str, target: &str, root: &Path) -> Result<String> {
    validate_component(lane, "lane")?;
    validate_component(profile, "profile")?;
    validate_component(target, "target")?;
    let rustc = Command::new("rustc")
        .arg("-vV")
        .output()
        .context("read Rust toolchain identity")?;
    require_success(&rustc, "read Rust toolchain identity")?;
    let rustflags = env::var("RUSTFLAGS").unwrap_or_default();
    let encoded_rustflags = env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default();
    let image = env::var("KITHARA_CI_PROVISIONED_LINUX_IMAGE").unwrap_or_default();
    let mut hash = Sha256::new();
    for value in [
        Snapshot::SCHEMA.as_bytes(),
        lane.as_bytes(),
        profile.as_bytes(),
        target.as_bytes(),
        &rustc.stdout,
        rustflags.as_bytes(),
        encoded_rustflags.as_bytes(),
        image.as_bytes(),
    ] {
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value);
    }
    for path in ["Cargo.lock", ".config/ci-pins.toml"] {
        let bytes =
            fs::read(root.join(path)).with_context(|| format!("read snapshot input {path}"))?;
        hash.update((path.len() as u64).to_le_bytes());
        hash.update(path.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    Ok(hex::encode(hash.finalize()))
}

fn publish(target: &Path, fingerprint: &str) -> Result<()> {
    ensure!(
        CacheTrust::from_environment()? == CacheTrust::Trusted,
        "only a trusted CI job may publish a target snapshot"
    );
    validate_fingerprint(fingerprint)?;
    require_target(target, false)?;
    let archive = NamedTempFile::new().context("create target snapshot archive")?;
    run_command(
        Command::new("tar")
            .args(["--create", "--zstd", "--file"])
            .arg(archive.path())
            .arg("--exclude=.kithara-ci-lease")
            .arg("--directory")
            .arg(target)
            .arg("."),
        "archive target snapshot",
    )?;
    let checksum = sha256(archive.path())?;
    let object = object(fingerprint, &checksum);
    let client = Client::load()?;
    if client.exists(&object)? {
        info!(%fingerprint, %checksum, "target snapshot already exists");
        return Ok(());
    }
    client.copy(archive.path(), &object)?;
    info!(%fingerprint, %checksum, "published immutable target snapshot");
    Ok(())
}

fn restore(target: &Path, fingerprint: &str) -> Result<()> {
    validate_fingerprint(fingerprint)?;
    require_target(target, true)?;
    let client = Client::load()?;
    let Some(object) = client.latest(fingerprint)? else {
        info!(%fingerprint, "no trusted target snapshot exists");
        return Ok(());
    };
    let expected = object
        .rsplit_once('/')
        .and_then(|(_, name)| name.strip_suffix(".tar"))
        .context("target snapshot object has no checksum name")?;
    let archive = NamedTempFile::new().context("create target snapshot download")?;
    client.copy_from(&object, archive.path())?;
    ensure!(
        sha256(archive.path())? == expected,
        "target snapshot checksum mismatch"
    );
    verify_archive(archive.path())?;
    run_command(
        Command::new("tar")
            .args(["--extract", "--zstd", "--file"])
            .arg(archive.path())
            .arg("--directory")
            .arg(target),
        "restore target snapshot",
    )?;
    info!(%fingerprint, object, "restored immutable target snapshot");
    Ok(())
}

fn require_target(target: &Path, may_create: bool) -> Result<()> {
    ensure!(
        target.is_absolute(),
        "target snapshot directory must be absolute"
    );
    if !target.exists() {
        ensure!(may_create, "target snapshot directory does not exist");
        fs::create_dir_all(target)
            .with_context(|| format!("create target snapshot directory {}", target.display()))?;
    }
    ensure!(target.is_dir(), "target snapshot path is not a directory");
    if may_create {
        ensure!(
            fs::read_dir(target)?.all(|entry| {
                let Ok(entry) = entry else {
                    return false;
                };
                entry.file_name() == lease::FILE
                    && entry.file_type().is_ok_and(|file_type| file_type.is_file())
            }),
            "target snapshot restore requires an empty private target directory except its job lease"
        );
    }
    Ok(())
}

fn object(fingerprint: &str, checksum: &str) -> String {
    format!("{}/{fingerprint}/{checksum}.tar", Snapshot::PREFIX)
}

fn validate_fingerprint(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "target snapshot fingerprint must be a lowercase SHA-256"
    );
    Ok(())
}

fn validate_component(value: &str, label: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')),
        "target snapshot {label} contains unsafe characters"
    );
    Ok(())
}

fn verify_archive(archive: &Path) -> Result<()> {
    let output = Command::new("tar")
        .args(["--list", "--zstd", "--file"])
        .arg(archive)
        .output()
        .context("list target snapshot archive")?;
    require_success(&output, "list target snapshot archive")?;
    for path in String::from_utf8(output.stdout)
        .context("target snapshot archive paths are not UTF-8")?
        .lines()
    {
        let path = Path::new(path);
        ensure!(
            !path.is_absolute()
                && path
                    .components()
                    .all(|component| matches!(component, Component::CurDir | Component::Normal(_))),
            "target snapshot archive contains unsafe path {}",
            path.display()
        );
    }
    Ok(())
}

fn sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("read {}", path.display()))?;
        if read == 0 {
            return Ok(hex::encode(hash.finalize()));
        }
        hash.update(&buffer[..read]);
    }
}

fn run_command(command: &mut Command, what: &str) -> Result<()> {
    let output = command.output().with_context(|| format!("start {what}"))?;
    require_success(&output, what)
}

fn require_success(output: &Output, what: &str) -> Result<()> {
    ensure!(
        output.status.success(),
        "{what} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

struct Client {
    endpoint: String,
    environment: BTreeMap<String, String>,
}

impl Client {
    fn load() -> Result<Self> {
        let environment = current_client_environment()?;
        let endpoint = environment
            .get("SCCACHE_ENDPOINT")
            .cloned()
            .context("cache environment has no endpoint")?;
        Ok(Self {
            endpoint,
            environment,
        })
    }

    fn command(&self) -> Result<Command> {
        let key = self
            .environment
            .get("AWS_ACCESS_KEY_ID")
            .context("cache key missing")?;
        let secret = self
            .environment
            .get("AWS_SECRET_ACCESS_KEY")
            .context("cache secret missing")?;
        let output = Command::new("mc")
            .args([
                "alias",
                "set",
                "snapshot",
                &self.endpoint,
                key,
                secret,
                "--api",
                "S3v4",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .context("configure snapshot storage client")?;
        require_success(&output, "configure snapshot storage client")?;
        Ok(Command::new("mc"))
    }

    fn exists(&self, object: &str) -> Result<bool> {
        let mut command = self.command()?;
        let status = command
            .arg("stat")
            .arg(Self::remote(object))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        Ok(status.success())
    }

    fn latest(&self, fingerprint: &str) -> Result<Option<String>> {
        let mut command = self.command()?;
        let output = command
            .args(["ls", "--json"])
            .arg(Self::remote(&format!(
                "{}/{fingerprint}/",
                Snapshot::PREFIX
            )))
            .output()?;
        require_success(&output, "list trusted target snapshots")?;
        let mut objects = String::from_utf8(output.stdout)
            .context("snapshot storage listing is not UTF-8")?
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter_map(|value| {
                value
                    .get("key")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .filter_map(|key| {
                let key = if key.starts_with(Snapshot::PREFIX) {
                    key
                } else {
                    format!("{}/{fingerprint}/{key}", Snapshot::PREFIX)
                };
                key.ends_with(".tar").then_some(key)
            })
            .collect::<Vec<_>>();
        objects.sort();
        Ok(objects.pop())
    }

    fn copy(&self, source: &Path, object: &str) -> Result<()> {
        let mut command = self.command()?;
        run_command(
            command.arg("cp").arg(source).arg(Self::remote(object)),
            "upload target snapshot",
        )
    }

    fn copy_from(&self, object: &str, destination: &Path) -> Result<()> {
        let mut command = self.command()?;
        run_command(
            command.arg("cp").arg(Self::remote(object)).arg(destination),
            "download target snapshot",
        )
    }

    fn remote(object: &str) -> String {
        format!("snapshot/{}/{object}", Snapshot::TRUSTED_BUCKET)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_snapshot_names_are_unambiguous_and_safe() {
        assert_eq!(
            object(&"a".repeat(64), &"b".repeat(64)),
            format!(
                "{Snapshot::PREFIX}/{}/{}.tar",
                "a".repeat(64),
                "b".repeat(64)
            )
        );
        assert!(validate_fingerprint(&"a".repeat(64)).is_ok());
        assert!(validate_fingerprint("../trusted").is_err());
        assert!(validate_component("audio", "lane").is_ok());
        assert!(validate_component("audio/../trusted", "lane").is_err());
    }

    #[test]
    fn fingerprint_changes_when_the_lockfile_changes() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(".config")).unwrap();
        fs::write(root.path().join("Cargo.lock"), "first").unwrap();
        fs::write(root.path().join(".config/ci-pins.toml"), "pins").unwrap();
        let first = fingerprint("audio", "test-release", "host", root.path()).unwrap();
        fs::write(root.path().join("Cargo.lock"), "second").unwrap();
        let second = fingerprint("audio", "test-release", "host", root.path()).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn restore_refuses_a_target_with_build_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        require_target(&target, true).unwrap();
        fs::write(target.join("artifact"), "compiled").unwrap();
        assert!(require_target(&target, true).is_err());
    }

    #[test]
    fn restore_keeps_the_job_lease_out_of_the_snapshot_payload() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        require_target(&target, true).unwrap();
        fs::write(target.join(lease::FILE), "held").unwrap();

        assert!(require_target(&target, true).is_ok());
    }

    #[test]
    fn an_archive_path_cannot_escape_the_target() {
        assert!(
            Path::new("./debug/libx.rlib")
                .components()
                .all(|component| matches!(component, Component::CurDir | Component::Normal(_)))
        );
        assert!(
            !Path::new("../trusted")
                .components()
                .all(|component| matches!(component, Component::CurDir | Component::Normal(_)))
        );
    }
}
