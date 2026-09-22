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
use crate::ci::build_cache::TARGET_HEARTBEAT_FILE;

struct Snapshot;

impl Snapshot {
    const SCHEMA: &str = "kithara-target-snapshot-v3";
    const PREFIX: &str = "target-snapshots";

    fn object(fingerprint: &str, checksum: &str) -> String {
        format!("{}/{fingerprint}/{checksum}.tar", Self::PREFIX)
    }
}

/// The dependency sources a job would otherwise fetch from the public
/// internet. Unlike a target snapshot this layer is content-addressed by
/// `Cargo.lock` alone: sources do not depend on the toolchain, the flags, the
/// lane or the checkout path, and they are identical on every platform, so one
/// object serves both fleets. It is published from the trusted scope and read
/// from there by every scope, which is what the bucket policy already admits.
struct Sources;

impl Sources {
    const SCHEMA: &str = "kithara-source-snapshot-v1";
    const PREFIX: &str = "source-snapshots";
    const BUCKET: &str = "kithara-trusted";
    /// Records which layer a `CARGO_HOME` already carries, so a job that
    /// already has it neither downloads it again nor unpacks over a live one.
    const MARKER: &str = ".kithara-source-snapshot";
    /// The directories carried, relative to `CARGO_HOME`. `registry` holds the
    /// index, the downloaded `.crate` files and their unpacked sources; `git`
    /// holds the bare databases and the checkouts cargo builds from.
    const PATHS: [&str; 5] = [
        "registry/cache",
        "registry/index",
        "registry/src",
        "git/db",
        "git/checkouts",
    ];

    fn object(fingerprint: &str, checksum: &str) -> String {
        format!("{}/{fingerprint}/{checksum}.tar", Self::PREFIX)
    }
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
    /// Restore an immutable snapshot into an empty private target directory.
    Restore {
        #[arg(long)]
        target: PathBuf,
        #[arg(long)]
        fingerprint: String,
    },
    /// Publish an immutable target snapshot in this job's cache scope.
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
            let cargo_home = env::var_os("CARGO_HOME").map_or_else(PathBuf::new, PathBuf::from);
            println!(
                "{}",
                fingerprint(lane, profile, target, Path::new("."), &cargo_home)?
            );
            Ok(())
        }
        SnapshotCommand::Restore {
            target,
            fingerprint,
        } => restore(target, fingerprint, Path::new("mc")).map(|_| ()),
        SnapshotCommand::Publish {
            target,
            fingerprint,
        } => publish(target, fingerprint, Path::new("mc")),
    }
}

pub(crate) fn restore_for_lane(
    key: &str,
    target: &Path,
    root: &Path,
    cargo_home: &Path,
    mc: &Path,
) -> Result<Option<String>> {
    let fingerprint = fingerprint(key, "cargo", "host", root, cargo_home)?;
    let restored = restore(target, &fingerprint, mc)?;
    Ok(snapshot_to_publish(fingerprint, restored))
}

fn snapshot_to_publish(fingerprint: String, restored: bool) -> Option<String> {
    (!restored).then_some(fingerprint)
}

pub(crate) fn publish_for_lane(target: &Path, fingerprint: &str, mc: &Path) -> Result<()> {
    publish(target, fingerprint, mc)
}

/// Fill `cargo_home` with the dependency sources this `Cargo.lock` names, so
/// the job compiles instead of fetching. Returns whether anything was restored.
pub(crate) fn restore_sources(root: &Path, cargo_home: &Path, mc: &Path) -> Result<bool> {
    let fingerprint = sources_fingerprint(root)?;
    if read_marker(cargo_home)?.as_deref() == Some(fingerprint.as_str()) {
        info!(%fingerprint, "source layer already present");
        return Ok(false);
    }
    let client = Client::load(mc)?;
    let Some(object) = client.latest(Sources::BUCKET, Sources::PREFIX, &fingerprint)? else {
        info!(%fingerprint, "no source snapshot exists");
        return Ok(false);
    };
    let expected = checksum_of(&object)?;
    let archive = NamedTempFile::new().context("create source snapshot download")?;
    client.copy_from(Sources::BUCKET, &object, archive.path())?;
    ensure!(
        sha256(archive.path())? == expected,
        "source snapshot checksum mismatch"
    );
    verify_archive(archive.path())?;
    fs::create_dir_all(cargo_home)
        .with_context(|| format!("create cargo home {}", cargo_home.display()))?;
    run_command(
        Command::new("tar")
            .args(["--extract", "--zstd", keep_existing()?, "--file"])
            .arg(archive.path())
            .arg("--directory")
            .arg(cargo_home),
        "restore source snapshot",
    )?;
    write_marker(cargo_home, &fingerprint)?;
    info!(%fingerprint, object, "restored dependency sources");
    Ok(true)
}

/// Publish the sources this job ended up with. Only the trusted scope may
/// write the bucket, so a branch that fetched something new leaves it for the
/// default branch to record rather than publishing its own.
pub(crate) fn publish_sources(root: &Path, cargo_home: &Path, mc: &Path) -> Result<()> {
    let fingerprint = sources_fingerprint(root)?;
    let present: Vec<&str> = Sources::PATHS
        .into_iter()
        .filter(|path| cargo_home.join(path).is_dir())
        .collect();
    ensure!(
        !present.is_empty(),
        "cargo home carries no sources to publish"
    );
    let archive = NamedTempFile::new().context("create source snapshot archive")?;
    let mut command = Command::new("tar");
    command
        .args(["--create", "--zstd", "--file"])
        .arg(archive.path())
        .arg("--directory")
        .arg(cargo_home);
    for path in present {
        command.arg(path);
    }
    run_command(&mut command, "archive source snapshot")?;
    let checksum = sha256(archive.path())?;
    let object = Sources::object(&fingerprint, &checksum);
    let client = Client::load(mc)?;
    if client.exists(Sources::BUCKET, &object)? {
        info!(%fingerprint, %checksum, "source snapshot already exists");
        return Ok(());
    }
    client.copy(archive.path(), Sources::BUCKET, &object)?;
    write_marker(cargo_home, &fingerprint)?;
    info!(%fingerprint, %checksum, "published dependency sources");
    Ok(())
}

/// The extraction flag that leaves a file already on disk alone.
///
/// Nothing in this archive is worth overwriting: a registry entry is named by
/// its content, so a file that is already there already holds the right bytes.
/// Leaving it is also what keeps the restore safe beside a neighbour, because
/// the two jobs this host runs at once share one cargo home and this untar does
/// not hold the package-cache lock the neighbour's compile respects. The two
/// tars disagree on which flag says it: the BSD one errors on `--skip-old-files`
/// and the GNU one treats `--keep-old-files` as a demand that nothing collide.
fn keep_existing() -> Result<&'static str> {
    let version = Command::new("tar")
        .arg("--version")
        .output()
        .context("read tar version")?;
    Ok(if version.stdout.starts_with(b"tar (GNU tar)") {
        "--skip-old-files"
    } else {
        "--keep-old-files"
    })
}

/// The lock file is the whole key. It names every crate version and every git
/// revision the build may reach, and nothing else about the job changes what
/// those bytes are.
fn sources_fingerprint(root: &Path) -> Result<String> {
    let lock = root.join("Cargo.lock");
    let bytes = fs::read(&lock).with_context(|| format!("read {}", lock.display()))?;
    let mut hash = Sha256::new();
    for value in [Sources::SCHEMA.as_bytes(), &bytes] {
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value);
    }
    Ok(hex::encode(hash.finalize()))
}

fn read_marker(cargo_home: &Path) -> Result<Option<String>> {
    let marker = cargo_home.join(Sources::MARKER);
    if !marker.is_file() {
        return Ok(None);
    }
    let value = fs::read_to_string(&marker)
        .with_context(|| format!("read {}", marker.display()))?
        .trim()
        .to_owned();
    Ok(Some(value))
}

fn write_marker(cargo_home: &Path, fingerprint: &str) -> Result<()> {
    let marker = cargo_home.join(Sources::MARKER);
    fs::write(&marker, fingerprint).with_context(|| format!("write {}", marker.display()))
}

/// The object name carries the checksum its bytes must have, which is what
/// makes a published snapshot immutable rather than merely named.
fn checksum_of(object: &str) -> Result<&str> {
    object
        .rsplit_once('/')
        .and_then(|(_, name)| name.strip_suffix(".tar"))
        .context("snapshot object has no checksum name")
}

fn fingerprint(
    lane: &str,
    profile: &str,
    target: &str,
    root: &Path,
    cargo_home: &Path,
) -> Result<String> {
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
    let mut hash = Sha256::new();
    for value in [
        Snapshot::SCHEMA.as_bytes(),
        lane.as_bytes(),
        profile.as_bytes(),
        target.as_bytes(),
        &rustc.stdout,
        rustflags.as_bytes(),
        encoded_rustflags.as_bytes(),
        root.as_os_str().as_encoded_bytes(),
        cargo_home.as_os_str().as_encoded_bytes(),
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

fn publish(target: &Path, fingerprint: &str, mc: &Path) -> Result<()> {
    validate_fingerprint(fingerprint)?;
    require_target(target, false)?;
    let archive = NamedTempFile::new().context("create target snapshot archive")?;
    run_command(
        Command::new("tar")
            .args(["--create", "--zstd", "--file"])
            .arg(archive.path())
            .arg("--exclude=.kithara-ci-lease")
            .arg(format!("--exclude={TARGET_HEARTBEAT_FILE}"))
            .arg("--directory")
            .arg(target)
            .arg("."),
        "archive target snapshot",
    )?;
    let checksum = sha256(archive.path())?;
    let object = Snapshot::object(fingerprint, &checksum);
    let client = Client::load(mc)?;
    if client.exists(&client.bucket, &object)? {
        info!(%fingerprint, %checksum, "target snapshot already exists");
        return Ok(());
    }
    client.copy(archive.path(), &client.bucket, &object)?;
    info!(%fingerprint, %checksum, "published immutable target snapshot");
    Ok(())
}

fn restore(target: &Path, fingerprint: &str, mc: &Path) -> Result<bool> {
    validate_fingerprint(fingerprint)?;
    require_target(target, true)?;
    let client = Client::load(mc)?;
    let Some(object) = client.latest(&client.bucket, Snapshot::PREFIX, fingerprint)? else {
        info!(%fingerprint, "no target snapshot exists");
        return Ok(false);
    };
    let expected = checksum_of(&object)?;
    let archive = NamedTempFile::new().context("create target snapshot download")?;
    client.copy_from(&client.bucket, &object, archive.path())?;
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
    Ok(true)
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
                matches!(
                    entry.file_name().to_str(),
                    Some(name) if name == lease::FILE || name == TARGET_HEARTBEAT_FILE
                ) && entry.file_type().is_ok_and(|file_type| file_type.is_file())
            }),
            "target snapshot restore requires an empty private target directory except its live job markers"
        );
    }
    Ok(())
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
    // Cargo fingerprints retain registry source paths under CARGO_HOME. The
    // snapshot therefore has to follow the same trust-scoped bucket as that
    // home or every restored dependency is immediately stale.
    bucket: String,
    endpoint: String,
    environment: BTreeMap<String, String>,
    program: PathBuf,
}

impl Client {
    fn load(program: &Path) -> Result<Self> {
        let environment = current_client_environment()?;
        let endpoint = environment
            .get("SCCACHE_ENDPOINT")
            .cloned()
            .context("cache environment has no endpoint")?;
        let bucket = environment
            .get("SCCACHE_BUCKET")
            .cloned()
            .context("cache environment has no bucket")?;
        Ok(Self {
            bucket,
            endpoint,
            environment,
            program: program.to_owned(),
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
        let output = Command::new(&self.program)
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
        Ok(Command::new(&self.program))
    }

    fn exists(&self, bucket: &str, object: &str) -> Result<bool> {
        let mut command = self.command()?;
        let status = command
            .arg("stat")
            .arg(Self::remote(bucket, object))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        Ok(status.success())
    }

    fn latest(&self, bucket: &str, prefix: &str, fingerprint: &str) -> Result<Option<String>> {
        let mut command = self.command()?;
        let output = command
            .args(["ls", "--json"])
            .arg(Self::remote(bucket, &format!("{prefix}/{fingerprint}/")))
            .output()?;
        require_success(&output, "list snapshots")?;
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
                let key = if key.starts_with(prefix) {
                    key
                } else {
                    format!("{prefix}/{fingerprint}/{key}")
                };
                key.ends_with(".tar").then_some(key)
            })
            .collect::<Vec<_>>();
        objects.sort();
        Ok(objects.pop())
    }

    fn copy(&self, source: &Path, bucket: &str, object: &str) -> Result<()> {
        let mut command = self.command()?;
        run_command(
            command
                .arg("cp")
                .arg(source)
                .arg(Self::remote(bucket, object)),
            "upload snapshot",
        )
    }

    fn copy_from(&self, bucket: &str, object: &str, destination: &Path) -> Result<()> {
        let mut command = self.command()?;
        run_command(
            command
                .arg("cp")
                .arg(Self::remote(bucket, object))
                .arg(destination),
            "download snapshot",
        )
    }

    /// Addresses a named bucket rather than only this job's own. The source
    /// layer lives in the trusted scope and is read from there by every scope,
    /// which is the access the bucket policy grants and nothing wider.
    fn remote(bucket: &str, object: &str) -> String {
        format!("snapshot/{bucket}/{object}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_snapshot_names_are_unambiguous_and_safe() {
        assert_eq!(
            Snapshot::object(&"a".repeat(64), &"b".repeat(64)),
            format!(
                "{}/{}/{}.tar",
                Snapshot::PREFIX,
                "a".repeat(64),
                "b".repeat(64)
            )
        );
        assert!(validate_fingerprint(&"a".repeat(64)).is_ok());
        assert!(validate_fingerprint("../trusted").is_err());
        assert!(validate_component("audio", "lane").is_ok());
        assert!(validate_component("audio/../trusted", "lane").is_err());
    }

    /// A source snapshot must be reachable from a scope that is not trusted,
    /// because that is the whole point: the default branch records the layer
    /// and every branch reads it. A client that could only address its own
    /// bucket sent a review job looking for an object only `main` ever writes.
    #[test]
    fn a_review_job_reads_sources_from_the_trusted_bucket() {
        let client = Client {
            bucket: "kithara-review".to_owned(),
            endpoint: String::new(),
            environment: BTreeMap::new(),
            program: PathBuf::from("mc"),
        };

        assert_eq!(
            Client::remote(Sources::BUCKET, &Sources::object("a", "b")),
            "snapshot/kithara-trusted/source-snapshots/a/b.tar"
        );
        assert_eq!(
            Client::remote(&client.bucket, "target-snapshots/a/b.tar"),
            "snapshot/kithara-review/target-snapshots/a/b.tar"
        );
    }

    /// The lock file is the entire key. A lane, a profile or a toolchain that
    /// changed the fingerprint would manufacture a miss for sources that are
    /// byte-identical, and the layer would go cold for no reason.
    /// A restore runs beside a neighbour that is compiling out of the same
    /// cargo home, so it must never rewrite a file that neighbour may have
    /// open. Both tars can say that; they disagree on the spelling, and the
    /// wrong one either fails the extraction or performs it destructively.
    #[test]
    fn a_restore_never_overwrites_what_the_cargo_home_already_holds() {
        let flag = keep_existing().unwrap();
        assert!(matches!(flag, "--skip-old-files" | "--keep-old-files"));
        let archive = NamedTempFile::new().unwrap();
        let home = tempfile::tempdir().unwrap();
        fs::write(home.path().join("kept"), b"original").unwrap();
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("kept"), b"replacement").unwrap();
        run_command(
            Command::new("tar")
                .args(["--create", "--file"])
                .arg(archive.path())
                .arg("--directory")
                .arg(source.path())
                .arg("kept"),
            "archive",
        )
        .unwrap();
        run_command(
            Command::new("tar")
                .args(["--extract", flag, "--file"])
                .arg(archive.path())
                .arg("--directory")
                .arg(home.path()),
            "extract",
        )
        .unwrap();
        assert_eq!(fs::read(home.path().join("kept")).unwrap(), b"original");
    }

    #[test]
    fn the_source_key_follows_the_lock_file_alone() {
        let first = tempfile::tempdir().expect("a temporary root");
        let second = tempfile::tempdir().expect("a temporary root");
        fs::write(first.path().join("Cargo.lock"), b"version = 4").expect("a lock file");
        fs::write(second.path().join("Cargo.lock"), b"version = 4").expect("a lock file");

        assert_eq!(
            sources_fingerprint(first.path()).expect("a fingerprint"),
            sources_fingerprint(second.path()).expect("a fingerprint"),
            "the same lock in a different checkout names the same sources"
        );

        fs::write(second.path().join("Cargo.lock"), b"version = 5").expect("a lock file");
        assert_ne!(
            sources_fingerprint(first.path()).expect("a fingerprint"),
            sources_fingerprint(second.path()).expect("a fingerprint"),
            "a changed lock must not serve the previous sources"
        );
    }

    #[test]
    fn a_source_layer_is_recognised_by_its_marker() {
        let home = tempfile::tempdir().expect("a temporary cargo home");

        assert_eq!(read_marker(home.path()).expect("a marker read"), None);
        write_marker(home.path(), "fingerprint").expect("a marker write");
        assert_eq!(
            read_marker(home.path()).expect("a marker read"),
            Some("fingerprint".to_owned())
        );
    }

    #[test]
    fn a_snapshot_object_names_the_checksum_its_bytes_must_have() {
        assert_eq!(
            checksum_of("source-snapshots/key/abc.tar").expect("a checksum"),
            "abc"
        );
        assert!(checksum_of("source-snapshots/key/abc").is_err());
    }

    #[test]
    fn target_snapshots_stay_in_the_compiler_cache_scope() {
        let client = Client {
            bucket: "kithara-review".to_owned(),
            endpoint: String::new(),
            environment: BTreeMap::new(),
            program: PathBuf::from("mc"),
        };

        assert_eq!(
            Client::remote(&client.bucket, "target-snapshots/fingerprint/archive.tar"),
            "snapshot/kithara-review/target-snapshots/fingerprint/archive.tar"
        );
    }

    #[test]
    fn a_restored_snapshot_is_not_published_again() {
        assert_eq!(snapshot_to_publish("hit".to_owned(), true), None);
        assert_eq!(
            snapshot_to_publish("miss".to_owned(), false),
            Some("miss".to_owned())
        );
    }

    #[test]
    fn fingerprint_changes_when_the_lockfile_changes() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(".config")).unwrap();
        fs::write(root.path().join("Cargo.lock"), "first").unwrap();
        fs::write(root.path().join(".config/ci-pins.toml"), "pins").unwrap();
        let cargo_home = root.path().join("cargo");
        let first = fingerprint("audio", "test-release", "host", root.path(), &cargo_home).unwrap();
        fs::write(root.path().join("Cargo.lock"), "second").unwrap();
        let second =
            fingerprint("audio", "test-release", "host", root.path(), &cargo_home).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn fingerprint_separates_non_portable_paths() {
        let first_root = tempfile::tempdir().unwrap();
        let second_root = tempfile::tempdir().unwrap();
        for root in [&first_root, &second_root] {
            fs::create_dir_all(root.path().join(".config")).unwrap();
            fs::write(root.path().join("Cargo.lock"), "lock").unwrap();
            fs::write(root.path().join(".config/ci-pins.toml"), "pins").unwrap();
        }

        let first = fingerprint(
            "audio",
            "test-release",
            "host",
            first_root.path(),
            Path::new("/cache/review/cargo"),
        )
        .unwrap();
        let second = fingerprint(
            "audio",
            "test-release",
            "host",
            first_root.path(),
            Path::new("/cache/quarantine/cargo"),
        )
        .unwrap();
        let third = fingerprint(
            "audio",
            "test-release",
            "host",
            second_root.path(),
            Path::new("/cache/review/cargo"),
        )
        .unwrap();

        assert_ne!(first, second);
        assert_ne!(first, third);
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
    fn restore_accepts_the_live_job_markers() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        require_target(&target, true).unwrap();
        fs::write(target.join(lease::FILE), "held").unwrap();
        fs::write(target.join(TARGET_HEARTBEAT_FILE), "alive").unwrap();

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
