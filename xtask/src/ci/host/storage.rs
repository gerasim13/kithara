use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use tracing::{info, warn};

use crate::ci::{config::CiConfig, process::Process};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
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
    runner: &'a str,
    timestamp: u64,
}

impl<'a> HostStorage<'a> {
    const ACTIVE_LEASE: Duration = Duration::from_secs(12 * 60 * 60);
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
        let pressure = self.pressure(used);
        match pressure {
            Pressure::Reject => bail!(
                "CI volume uses {used} bytes; new jobs stop at {}",
                self.config.host.reject_bytes
            ),
            Pressure::Soft | Pressure::Aggressive => {
                warn!(used_bytes = used, ?pressure, "CI volume is under pressure");
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
        let pressure = self.pressure(initial);
        info!(used_bytes = initial, ?pressure, "cleanup started");

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

        let mut final_used = self.used_bytes()?;
        if final_used >= self.config.host.reject_bytes {
            self.prune_old_trees("cache/trusted", Duration::ZERO)?;
            self.prune_old_trees("cache/bootstrap/trusted", Duration::ZERO)?;
            self.prune_retired_caches(Duration::ZERO)?;
            final_used = self.used_bytes()?;
        }
        if final_used >= self.config.host.reject_bytes {
            // Every step above works on what jobs leave behind, and none of it
            // is where the space went: the macOS guest grows on this volume for
            // as long as it lives and only gives the space back when it is
            // thrown away — measured at 38 gibibytes in one recycle, against
            // three and a half for everything else together. Restarting the
            // agent is what an operator does by hand here, and it costs the
            // job now in flight. That trade only makes sense once jobs are
            // being refused anyway, which is exactly this branch.
            self.recycle_macos_guest();
            final_used = self.used_bytes()?;
        }
        let final_pressure = self.pressure(final_used);
        info!(
            used_bytes = final_used,
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
        let pressure = self.pressure(used);
        let runner = self.runner_state();
        serde_json::to_writer(
            io::stdout().lock(),
            &Health {
                volume_used_bytes: used,
                pressure,
                runner,
                timestamp: unix_time()?,
            },
        )
        .context("writing host health JSON")?;
        io::stdout()
            .write_all(b"\n")
            .context("terminating host health JSON")?;
        if pressure == Pressure::Reject {
            bail!("CI volume is above the new-job threshold");
        }
        if runner == "stopped" {
            bail!("GitLab runner service is stopped");
        }
        Ok(())
    }

    fn used_bytes(&self) -> Result<u64> {
        let total = fs4::total_space(&self.root)
            .with_context(|| format!("reading total space for {}", self.root.display()))?;
        let available = fs4::available_space(&self.root)
            .with_context(|| format!("reading available space for {}", self.root.display()))?;
        Ok(total.saturating_sub(available))
    }

    fn pressure(&self, used: u64) -> Pressure {
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

    /// Restart the macOS runner agent, which throws its guest away and clones
    /// a fresh one from the base image.
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

pub(super) fn pressure_for(used: u64, soft: u64, aggressive: u64, reject: u64) -> Pressure {
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
