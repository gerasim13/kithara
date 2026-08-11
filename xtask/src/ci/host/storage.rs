use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use tracing::{info, warn};

use crate::ci::{build_cache, config::CiConfig, process::Process};

/// Ordered least to most urgent: the watchdog reports the worst volume, not
/// the total, so that a roomy one cannot hide a full one.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Pressure {
    Normal,
    Soft,
    Aggressive,
    Reject,
}

pub(super) struct HostStorage<'a> {
    root: PathBuf,
    config: &'a CiConfig,
    process: &'a Process,
}

#[derive(Serialize)]
struct Health<'a> {
    volume_used_bytes: u64,
    pressure: Pressure,
    /// Named so an operator can see which volume is under pressure without
    /// running `df` by hand.
    volumes: Vec<VolumeHealth>,
    runner: &'a str,
    timestamp: u64,
}

#[derive(Serialize)]
struct VolumeHealth {
    path: String,
    used_bytes: u64,
    total_bytes: u64,
    pressure: Pressure,
}

struct Volume {
    path: PathBuf,
    used: u64,
    total: u64,
}

impl Volume {
    fn read(path: &Path) -> Result<Self> {
        let total = fs4::total_space(path)
            .with_context(|| format!("reading total space for {}", path.display()))?;
        let available = fs4::available_space(path)
            .with_context(|| format!("reading available space for {}", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
            used: total.saturating_sub(available),
            total,
        })
    }
}

impl<'a> HostStorage<'a> {
    const ACTIVE_LEASE: Duration = Duration::from_secs(12 * 60 * 60);
    /// The profile `install-services` starts the Linux guest under.
    const COLIMA_PROFILE: &'static str = "kithara";
    const DAY: Duration = Duration::from_secs(24 * 60 * 60);
    const LOG_LIMIT_BYTES: u64 = 20_000_000;
    const REMOVABLE_ROOTS: &'static [&'static str] = &["cache", "logs", "vm", "workspaces"];
    /// The cache namespaces this repository still writes to.
    ///
    /// The steps below prune by name, so a namespace that stops being written
    /// to becomes invisible rather than stale, and nothing ever comes back for
    /// it. Six gigabytes of `cargo-reapi` stores sat here after that tool came
    /// off the CI path. Anything not named here is pruned on its own age.
    const CACHE_NAMESPACES: &'static [&'static str] = &[
        "bootstrap",
        "gitlab-runner",
        "quarantine",
        "review",
        "trusted",
    ];

    pub(super) fn new(config: &'a CiConfig, process: &'a Process) -> Result<Self> {
        let root = config.host.host_root.clone();
        validate_root(&root)?;
        Ok(Self {
            root,
            config,
            process,
        })
    }

    #[cfg(test)]
    fn for_test(root: PathBuf, config: &'a CiConfig, process: &'a Process) -> Result<Self> {
        validate_root(&root)?;
        Ok(Self {
            root,
            config,
            process,
        })
    }

    pub(super) fn preflight(&self) -> Result<()> {
        let used = self.used_bytes()?;
        let (pressure, volume) = self.worst_pressure()?;
        match pressure {
            Pressure::Reject => bail!("{} is full; new jobs stop here", volume.display()),
            Pressure::Soft | Pressure::Aggressive => {
                warn!(volume = %volume.display(), ?pressure, "CI volume is under pressure");
            }
            Pressure::Normal => {}
        }

        for name in ["cache", "logs", "toolchains", "vm", "workspaces"] {
            let directory = self.root.join(name);
            if !directory.is_dir() {
                bail!("missing CI directory: {}", directory.display());
            }
            writable_probe(&directory)?;
        }
        self.process.require_tools(&["git", "sccache"])?;
        info!(used_bytes = used, ?pressure, "host preflight passed");
        Ok(())
    }

    pub(super) fn cleanup(&self) -> Result<()> {
        let initial = self.used_bytes()?;
        // The worst single volume, not the sum: adding a second volume's bytes
        // to the first and comparing that against thresholds calibrated for the
        // first reads every machine with a guest volume as full, and the branch
        // it reaches for throws away the compiler caches that were never the
        // problem. `preflight` already decides this way.
        let (pressure, volume) = self.worst_pressure()?;
        info!(used_bytes = initial, ?pressure, volume = %volume.display(), "cleanup started");

        self.prune_old_trees("workspaces/tmp", Self::DAY)?;
        self.prune_old_trees("workspaces/builds", Self::DAY)?;
        self.prune_old_trees("workspaces/gitlab", Self::DAY)?;
        self.prune_old_trees("vm/overlays", Self::DAY)?;
        self.prune_old_trees("vm/android/avd", Self::DAY)?;
        self.prune_old_files("logs", 14 * Self::DAY)?;
        self.rotate_logs()?;
        self.prune_retired_caches(7 * Self::DAY)?;

        match pressure {
            Pressure::Soft => {
                self.prune_old_trees("cache/quarantine", 7 * Self::DAY)?;
                self.prune_old_trees("cache/review", 30 * Self::DAY)?;
                self.prune_old_trees("cache/bootstrap/quarantine", 7 * Self::DAY)?;
                self.prune_old_trees("cache/bootstrap/review", 30 * Self::DAY)?;
                self.prune_docker_cache("720h");
            }
            Pressure::Aggressive | Pressure::Reject => {
                self.prune_old_trees("cache/quarantine", Duration::ZERO)?;
                self.prune_old_trees("cache/review", Duration::ZERO)?;
                self.prune_old_trees("cache/bootstrap/quarantine", Duration::ZERO)?;
                self.prune_old_trees("cache/bootstrap/review", Duration::ZERO)?;
                self.prune_old_trees("cache/trusted", 7 * Self::DAY)?;
                self.prune_old_trees("cache/bootstrap/trusted", 7 * Self::DAY)?;
                self.prune_old_trees("vm/tart/cache", 7 * Self::DAY)?;
                self.prune_docker_cache("168h");
            }
            Pressure::Normal => {}
        }

        let target_dirs = persistent_target_dirs(&self.root.join("workspaces/gitlab"))?;
        build_cache::enforce_budget(&target_dirs, self.config.host.build_cache_budget_bytes()?)?;

        let (mut final_pressure, _) = self.worst_pressure()?;
        if final_pressure == Pressure::Reject {
            // The guests are where the space is, and the caches are what the
            // steps above can reach — so taking the caches first pays for the
            // guests with the compiler output the machine exists to keep warm.
            // Both guests only give their space back when they are thrown
            // away: the macOS one measured 38 gibibytes in a recycle against
            // three and a half for everything else together, and the Linux
            // one's disk is allocated once and never deflates, so pruning
            // inside it returns nothing to this volume. Recycling costs the
            // job in flight and a cold image build, which is a trade worth
            // making only once jobs are being refused anyway — this branch.
            self.recycle_linux_guest();
            self.recycle_macos_guest();
            final_pressure = self.worst_pressure()?.0;
        }
        if final_pressure == Pressure::Reject {
            self.prune_old_trees("cache/trusted", Duration::ZERO)?;
            self.prune_old_trees("cache/bootstrap/trusted", Duration::ZERO)?;
            self.prune_retired_caches(Duration::ZERO)?;
            final_pressure = self.worst_pressure()?.0;
        }
        info!(
            used_bytes = self.used_bytes()?,
            ?final_pressure,
            "cleanup completed"
        );
        if final_pressure == Pressure::Reject {
            bail!("CI volume remains above the new-job threshold after cleanup");
        }
        Ok(())
    }

    pub(super) fn health(&self) -> Result<()> {
        let used = self.used_bytes()?;
        let volumes: Vec<VolumeHealth> = self
            .volumes()?
            .into_iter()
            .map(|volume| VolumeHealth {
                path: volume.path.display().to_string(),
                used_bytes: volume.used,
                total_bytes: volume.total,
                pressure: self.pressure_of(&volume),
            })
            .collect();
        let (pressure, worst) = self.worst_pressure()?;
        let runner = self.runner_state();
        serde_json::to_writer(
            io::stdout().lock(),
            &Health {
                volume_used_bytes: used,
                pressure,
                volumes,
                runner,
                timestamp: unix_time()?,
            },
        )
        .context("writing host health JSON")?;
        io::stdout()
            .write_all(b"\n")
            .context("terminating host health JSON")?;
        if pressure == Pressure::Reject {
            bail!("{} is above the new-job threshold", worst.display());
        }
        if runner == "stopped" {
            bail!("GitLab runner service is stopped");
        }
        Ok(())
    }

    fn used_bytes(&self) -> Result<u64> {
        Ok(self.volumes()?.into_iter().map(|volume| volume.used).sum())
    }

    /// Every volume CI storage sits on, in the order they should be reported.
    ///
    /// The guest images can be given a volume of their own, so that one guest
    /// growing cannot refuse work for lanes that never touch it. `df` on the
    /// checkout root then stops seeing them entirely, and a watchdog blind to
    /// the largest consumer on the machine is worse than one watching a single
    /// shared volume. Anything reached through `vm` that turns out to live on
    /// another filesystem is measured as well.
    fn volumes(&self) -> Result<Vec<Volume>> {
        let mut volumes = vec![Volume::read(&self.root)?];
        let guests = self.root.join("vm");
        if guests.is_dir()
            && let Ok(guests) = guests.canonicalize()
            && !guests.starts_with(&self.root)
        {
            volumes.push(Volume::read(&guests)?);
        }
        Ok(volumes)
    }

    /// The worst any single volume is doing, and which one that is.
    ///
    /// Summing them would let a roomy volume hide a full one, which is the
    /// failure this split exists to prevent.
    fn worst_pressure(&self) -> Result<(Pressure, PathBuf)> {
        let volumes = self.volumes()?;
        let mut worst = (Pressure::Normal, self.root.clone());
        for volume in volumes {
            let pressure = self.pressure_of(&volume);
            if pressure > worst.0 {
                worst = (pressure, volume.path);
            }
        }
        Ok(worst)
    }

    /// Thresholds are configured against the main volume. Any other volume is
    /// held to the same proportions of its own capacity, so a second volume
    /// needs no second set of numbers to go stale.
    fn pressure_of(&self, volume: &Volume) -> Pressure {
        let quota = self.config.host.quota_bytes;
        if volume.path == self.root || volume.total == 0 || quota == 0 {
            return self.pressure(volume.used);
        }
        let scale = |bytes: u64| -> u64 {
            (u128::from(bytes) * u128::from(volume.total) / u128::from(quota))
                .try_into()
                .unwrap_or(u64::MAX)
        };
        pressure_for(
            volume.used,
            scale(self.config.host.soft_cleanup_bytes),
            scale(self.config.host.aggressive_cleanup_bytes),
            scale(self.config.host.reject_bytes),
        )
    }

    const fn pressure(&self, used: u64) -> Pressure {
        pressure_for(
            used,
            self.config.host.soft_cleanup_bytes,
            self.config.host.aggressive_cleanup_bytes,
            self.config.host.reject_bytes,
        )
    }

    /// Cache namespaces nothing writes to any more, once they have gone quiet
    /// for a week.
    fn prune_retired_caches(&self, age: Duration) -> Result<()> {
        let directory = self.root.join("cache");
        if !directory.is_dir() {
            return Ok(());
        }
        for entry in fs::read_dir(&directory)
            .with_context(|| format!("reading cache directory {}", directory.display()))?
        {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if Self::CACHE_NAMESPACES.contains(&name.as_ref()) {
                continue;
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.file_type().is_dir() || !older_than(&metadata, age)? {
                continue;
            }
            if self.active(&path) {
                info!(path = %path.display(), "keeping active CI path");
                continue;
            }
            info!(path = %path.display(), "removing retired cache namespace");
            self.remove_path(&path)?;
        }
        Ok(())
    }

    fn prune_old_trees(&self, relative: &str, age: Duration) -> Result<()> {
        let directory = self.root.join(relative);
        if !directory.is_dir() {
            return Ok(());
        }
        for entry in fs::read_dir(&directory)
            .with_context(|| format!("reading cleanup directory {}", directory.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.file_type().is_dir() || !older_than(&metadata, age)? {
                continue;
            }
            if self.active(&path) {
                info!(path = %path.display(), "keeping active CI path");
                continue;
            }
            self.remove_path(&path)?;
        }
        Ok(())
    }

    fn prune_old_files(&self, relative: &str, age: Duration) -> Result<()> {
        let directory = self.root.join(relative);
        if !directory.is_dir() {
            return Ok(());
        }
        self.prune_old_files_recursive(&directory, age)
    }

    fn prune_old_files_recursive(&self, directory: &Path, age: Duration) -> Result<()> {
        let mut subdirectories = Vec::new();
        for entry in fs::read_dir(directory)
            .with_context(|| format!("reading cleanup directory {}", directory.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                if older_than(&metadata, age)? {
                    self.remove_path(&path)?;
                }
            } else if metadata.is_dir() {
                self.prune_old_files_recursive(&path, age)?;
                subdirectories.push(path);
            } else if metadata.is_file() && older_than(&metadata, age)? {
                self.remove_path(&path)?;
            }
        }
        for directory in subdirectories {
            if fs::read_dir(&directory)?.next().is_none() {
                self.remove_path(&directory)?;
            }
        }
        Ok(())
    }

    fn rotate_logs(&self) -> Result<()> {
        let directory = self.root.join("logs");
        if !directory.is_dir() {
            return Ok(());
        }
        for entry in fs::read_dir(&directory)
            .with_context(|| format!("reading log directory {}", directory.display()))?
        {
            let path = entry?.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("log")
                || fs::symlink_metadata(&path)?.len() <= Self::LOG_LIMIT_BYTES
            {
                continue;
            }
            let numbered = |number| PathBuf::from(format!("{}.{number}", path.display()));
            self.remove_path(&numbered(5))?;
            for number in (1..=4).rev() {
                let source = numbered(number);
                if source.exists() {
                    fs::rename(&source, numbered(number + 1))
                        .with_context(|| format!("rotating log {}", source.display()))?;
                }
            }
            fs::copy(&path, numbered(1))
                .with_context(|| format!("copying rotated log {}", path.display()))?;
            OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&path)
                .with_context(|| format!("truncating rotated log {}", path.display()))?;
        }
        Ok(())
    }

    fn active(&self, path: &Path) -> bool {
        let marker = path.join(".kithara-ci-active");
        if let Ok(metadata) = fs::metadata(&marker) {
            if !older_than(&metadata, Self::ACTIVE_LEASE).unwrap_or(true) {
                return true;
            }
            warn!(path = %marker.display(), "removing stale CI cache lease");
            let _ = fs::remove_file(marker);
        }
        self.process
            .command("/usr/sbin/lsof")
            .arg("+D")
            .arg(path)
            .output()
            .is_ok_and(|output| output.status.success())
    }

    fn remove_path(&self, path: &Path) -> Result<()> {
        if !Self::is_removable(&self.root, path) {
            bail!("refusing to remove unsafe CI path: {}", path.display());
        }
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", path.display()));
            }
        };
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            fs::remove_dir_all(path).with_context(|| format!("removing {}", path.display()))
        } else {
            fs::remove_file(path).with_context(|| format!("removing {}", path.display()))
        }
    }

    fn prune_docker_cache(&self, age: &str) {
        let home = self.root.join("home").join(&self.config.host.ci_user);
        let socket = home.join(".colima/kithara/docker.sock");
        let docker = self.config.host.brew_tool("docker");
        if !socket.exists() || !docker.is_file() {
            return;
        }
        let mut command = self.process.command(docker);
        command
            .env("DOCKER_HOST", format!("unix://{}", socket.display()))
            .args(["builder", "prune", "--force", "--filter"])
            .arg(format!("until={age}"));
        if let Err(error) = self
            .process
            .run_command(&mut command, "Docker build cache cleanup")
        {
            warn!(%error, "Docker cache cleanup failed");
        }
    }

    /// Delete the Linux guest and the disk it kept, so the space is allocated
    /// again from nothing.
    ///
    /// The agent runs `colima start` in the foreground, so the process ends
    /// with the guest and launchd starts a fresh one. The images inside are
    /// rebuilt, which is the cost.
    ///
    /// Deleting the instance alone reclaims almost nothing: lima keeps the data
    /// disk as a named volume so that it survives a recreated guest, and the
    /// disk is where the space goes. It is sparse, so it grows with every write
    /// and never shrinks when the guest deletes. On 2026-08-07 the instance held
    /// 2 GB and the disk held 95 GB.
    ///
    /// Both are attempted even if the first fails: with the instance already
    /// gone, deleting the disk is exactly what still has to happen.
    fn recycle_linux_guest(&self) {
        let colima = self.config.host.brew_tool("colima");
        if !colima.is_file() {
            return;
        }
        info!("recycling the Linux guest to reclaim volume space");
        if let Err(error) = self.process.run(
            &colima.display().to_string(),
            &["delete", "--force", "--profile", Self::COLIMA_PROFILE],
            "recycle the Linux guest",
        ) {
            warn!(%error, "could not recycle the Linux guest");
        }

        let limactl = self.config.host.brew_tool("limactl");
        if !limactl.is_file() {
            warn!("limactl is absent, so the guest's data disk stays allocated");
            return;
        }
        let home = self.root.join("home").join(&self.config.host.ci_user);
        let mut command = self.process.command(limactl);
        command.env("LIMA_HOME", home.join(".colima/_lima")).args([
            "disk",
            "delete",
            &Self::linux_guest_disk(),
        ]);
        if let Err(error) = self
            .process
            .run_command(&mut command, "delete the Linux guest's data disk")
        {
            warn!(%error, "could not delete the Linux guest's data disk");
        }
    }

    /// colima names the disk after the profile it belongs to.
    fn linux_guest_disk() -> String {
        format!("colima-{}", Self::COLIMA_PROFILE)
    }

    fn recycle_macos_guest(&self) {
        let uid = self
            .process
            .capture("id", &["-u"], "current user id")
            .unwrap_or_default();
        let label = format!("gui/{uid}/com.zvuk.kithara-ci.macos-runner");
        info!(label, "recycling the macOS guest to reclaim volume space");
        if let Err(error) = self.process.run(
            "/bin/launchctl",
            &["kickstart", "-k", &label],
            "recycle the macOS guest",
        ) {
            warn!(%error, "could not recycle the macOS guest");
        }
    }

    fn runner_state(&self) -> &'static str {
        if !self.config.host.brew_tool("gitlab-runner").is_file() {
            return "not-installed";
        }
        let uid = self
            .process
            .capture("id", &["-u"], "current user id")
            .unwrap_or_default();
        let label = format!("gui/{uid}/com.zvuk.kithara-ci.gitlab-runner");
        if self
            .process
            .command("launchctl")
            .args(["print", &label])
            .output()
            .is_ok_and(|output| output.status.success())
        {
            "running"
        } else {
            "stopped"
        }
    }

    fn is_removable(root: &Path, target: &Path) -> bool {
        if !root.is_absolute() || !target.is_absolute() {
            return false;
        }
        let Ok(relative) = target.strip_prefix(root) else {
            return false;
        };
        let mut components = relative.components();
        let Some(Component::Normal(first)) = components.next() else {
            return false;
        };
        Self::REMOVABLE_ROOTS
            .iter()
            .any(|allowed| first == std::ffi::OsStr::new(allowed))
            && components.next().is_some()
            && relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
    }
}

pub(super) const fn pressure_for(used: u64, soft: u64, aggressive: u64, reject: u64) -> Pressure {
    if used >= reject {
        Pressure::Reject
    } else if used >= aggressive {
        Pressure::Aggressive
    } else if used >= soft {
        Pressure::Soft
    } else {
        Pressure::Normal
    }
}

fn persistent_target_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }

    let mut pending = vec![root.to_path_buf()];
    let mut targets = Vec::new();
    while let Some(directory) = pending.pop() {
        if directory.join("Cargo.toml").is_file() {
            for name in ["target", "target-flash-off"] {
                let path = directory.join(name);
                let metadata = match fs::symlink_metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(error).with_context(|| format!("reading {}", path.display()));
                    }
                };
                if metadata.file_type().is_dir() {
                    targets.push(path);
                }
            }
            continue;
        }

        let entries = fs::read_dir(&directory)
            .with_context(|| format!("reading CI workspace directory {}", directory.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| {
                format!(
                    "reading an entry in CI workspace directory {}",
                    directory.display()
                )
            })?;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .with_context(|| format!("reading CI workspace metadata for {}", path.display()))?;
            if metadata.file_type().is_dir() {
                pending.push(path);
            }
        }
    }
    targets.sort();
    Ok(targets)
}

fn validate_root(root: &Path) -> Result<()> {
    if !root.is_absolute() || !root.is_dir() {
        bail!("CI root is not mounted: {}", root.display());
    }
    if fs::symlink_metadata(root)?.file_type().is_symlink() {
        bail!("CI root must not be a symlink: {}", root.display());
    }
    Ok(())
}

fn older_than(metadata: &fs::Metadata, age: Duration) -> Result<bool> {
    let modified = metadata.modified().context("reading modification time")?;
    Ok(SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default()
        > age)
}

fn writable_probe(directory: &Path) -> Result<()> {
    let path = directory.join(format!(".kithara-write-probe-{}", std::process::id()));
    let result = fs::write(&path, b"probe")
        .with_context(|| format!("CI directory is not writable: {}", directory.display()));
    let cleanup =
        fs::remove_file(&path).with_context(|| format!("removing write probe {}", path.display()));
    result.and(cleanup)
}

fn unix_time() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::ci::config::fixture;

    fn config(root: &Path) -> CiConfig {
        let mut config = fixture();
        config.host.host_root = root.to_path_buf();
        config.host.cache_root_macos = root.join("cache");
        config.host.cache_root_linux = root.join("cache");
        config.host.cache_root_windows = root.join("cache");
        config.host.quota_bytes = 300;
        config.host.reject_bytes = 285;
        config.host.aggressive_cleanup_bytes = 270;
        config.host.soft_cleanup_bytes = 240;
        config
    }

    #[test]
    fn pressure_thresholds_are_exact() {
        assert_eq!(pressure_for(239, 240, 270, 285), Pressure::Normal);
        assert_eq!(pressure_for(240, 240, 270, 285), Pressure::Soft);
        assert_eq!(pressure_for(270, 240, 270, 285), Pressure::Aggressive);
        assert_eq!(pressure_for(285, 240, 270, 285), Pressure::Reject);
    }

    #[test]
    fn a_second_volume_is_held_to_the_same_proportions_of_its_own_size() {
        let directory = tempfile::tempdir().unwrap();
        let cfg = config(directory.path());
        let process = Process::new(directory.path(), BTreeMap::new());
        let storage =
            HostStorage::for_test(directory.path().to_path_buf(), &cfg, &process).unwrap();
        // Thresholds are 240/270/285 against a 300-byte main volume, so a
        // volume half that size refuses at 142 and is content at 119.
        let half = |used| Volume {
            path: PathBuf::from("/elsewhere"),
            used,
            total: 150,
        };

        assert_eq!(storage.pressure_of(&half(119)), Pressure::Normal);
        assert_eq!(storage.pressure_of(&half(120)), Pressure::Soft);
        assert_eq!(storage.pressure_of(&half(135)), Pressure::Aggressive);
        assert_eq!(storage.pressure_of(&half(143)), Pressure::Reject);
    }

    #[test]
    fn a_full_volume_is_not_hidden_by_a_roomy_one() {
        assert!(Pressure::Reject > Pressure::Normal);
        assert!(Pressure::Aggressive > Pressure::Soft);
    }

    #[test]
    fn a_namespace_nothing_writes_to_is_pruned_and_the_live_ones_are_not() {
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path().join("cache");
        for name in [
            "trusted",
            "review",
            "gitlab-runner",
            "reapi",
            "reapi-sccache",
        ] {
            fs::create_dir_all(cache.join(name)).unwrap();
        }
        let cfg = config(directory.path());
        let process = Process::new(directory.path(), BTreeMap::new());
        let storage =
            HostStorage::for_test(directory.path().to_path_buf(), &cfg, &process).unwrap();

        storage.prune_retired_caches(Duration::ZERO).unwrap();

        for name in ["trusted", "review", "gitlab-runner"] {
            assert!(cache.join(name).is_dir(), "{name} is still written to");
        }
        for name in ["reapi", "reapi-sccache"] {
            assert!(!cache.join(name).exists(), "{name} is retired");
        }
    }

    #[test]
    fn a_retired_namespace_survives_until_it_has_gone_quiet() {
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path().join("cache");
        fs::create_dir_all(cache.join("reapi")).unwrap();
        let cfg = config(directory.path());
        let process = Process::new(directory.path(), BTreeMap::new());
        let storage =
            HostStorage::for_test(directory.path().to_path_buf(), &cfg, &process).unwrap();

        storage.prune_retired_caches(HostStorage::DAY).unwrap();

        assert!(cache.join("reapi").is_dir());
    }

    #[test]
    fn cleanup_never_leaves_the_ci_root() {
        let directory = tempfile::tempdir().unwrap();
        for name in HostStorage::REMOVABLE_ROOTS {
            fs::create_dir_all(directory.path().join(name)).unwrap();
        }
        let cfg = config(directory.path());
        let process = Process::new(directory.path(), BTreeMap::new());
        let storage =
            HostStorage::for_test(directory.path().to_path_buf(), &cfg, &process).unwrap();
        let safe = directory.path().join("workspaces/tmp/old");
        fs::create_dir_all(&safe).unwrap();
        storage.remove_path(&safe).unwrap();
        assert!(!safe.exists());

        let outside = directory.path().parent().unwrap().join("outside");
        assert!(storage.remove_path(&outside).is_err());
    }

    #[test]
    fn active_marker_pins_a_workspace() {
        let directory = tempfile::tempdir().unwrap();
        for name in HostStorage::REMOVABLE_ROOTS {
            fs::create_dir_all(directory.path().join(name)).unwrap();
        }
        let cfg = config(directory.path());
        let process = Process::new(directory.path(), BTreeMap::new());
        let storage =
            HostStorage::for_test(directory.path().to_path_buf(), &cfg, &process).unwrap();
        let workspace = directory.path().join("workspaces/tmp/current");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join(".kithara-ci-active"), b"").unwrap();
        assert!(storage.active(&workspace));
    }
}
