use std::{
    collections::BTreeMap,
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
    host_root: PathBuf,
    build_root: PathBuf,
    config: &'a CiConfig,
    process: &'a Process,
}

struct Agents;

impl Agents {
    const LAUNCHCTL: &'static str = "/bin/launchctl";
    const RUNNING: &'static str = "running";
    /// A host with no launchd has no agents to be wrong about, and the Linux
    /// executor runs this same command.
    const ABSENT: &'static str = "not-applicable";
    /// The agents that must hold a process for work to reach this host.
    ///
    /// `cleanup` and `health` are periodic and spend nearly all their life
    /// loaded with nothing running, so a missing process says nothing about
    /// them. These three are `KeepAlive`, and a missing process means work has
    /// stopped.
    const ALWAYS_ON: &'static [&'static str] = &["colima", "gitlab-runner", "macos-runner"];
}

#[derive(Serialize)]
struct Health<'a> {
    /// What is left where there is least of it. The thresholds are free-space
    /// floors, so this is the number `pressure` was read from — reporting bytes
    /// spent instead described a quantity no decision uses.
    free_bytes: u64,
    pressure: Pressure,
    /// Named so an operator can see which volume is under pressure without
    /// running `df` by hand.
    volumes: Vec<VolumeHealth>,
    /// Every agent that has to be running for work to reach this host, by label
    /// suffix. Watching only the `gitlab-runner` agent missed the one whose death
    /// stops the work without stopping anything else.
    agents: BTreeMap<&'a str, &'static str>,
    timestamp: u64,
}

#[derive(Serialize)]
struct VolumeHealth {
    path: String,
    free_bytes: u64,
    total_bytes: u64,
    pressure: Pressure,
}

struct Volume {
    path: PathBuf,
    total: u64,
    available: u64,
}

impl Volume {
    fn read(path: &Path) -> Result<Self> {
        let total = fs4::total_space(path)
            .with_context(|| format!("reading total space for {}", path.display()))?;
        let available = fs4::available_space(path)
            .with_context(|| format!("reading available space for {}", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
            total,
            available,
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
        let host_root = config.host.host_root.clone();
        let build_root = config.host.build_root().to_path_buf();
        validate_root(&host_root)?;
        validate_root(&build_root)?;
        Ok(Self {
            host_root,
            build_root,
            config,
            process,
        })
    }

    #[cfg(test)]
    fn for_test(config: &'a CiConfig, process: &'a Process) -> Result<Self> {
        let host_root = config.host.host_root.clone();
        let build_root = config.host.build_root().to_path_buf();
        validate_root(&host_root)?;
        validate_root(&build_root)?;
        Ok(Self {
            host_root,
            build_root,
            config,
            process,
        })
    }

    pub(super) fn preflight(&self) -> Result<()> {
        let free = self.free_bytes()?;
        let (pressure, volume) = self.worst_pressure()?;
        match pressure {
            Pressure::Reject => bail!("{} is full; new jobs stop here", volume.display()),
            Pressure::Soft | Pressure::Aggressive => {
                warn!(volume = %volume.display(), ?pressure, "CI volume is under pressure");
            }
            Pressure::Normal => {}
        }

        for name in ["cache", "logs", "toolchains", "vm", "workspaces"] {
            let directory = self.host_root.join(name);
            if !directory.is_dir() {
                bail!("missing CI directory: {}", directory.display());
            }
            writable_probe(&directory)?;
        }
        let workspaces = self.build_root.join("workspaces");
        if !workspaces.is_dir() {
            bail!("missing CI directory: {}", workspaces.display());
        }
        writable_probe(&workspaces)?;
        self.process.require_tools(&["git", "sccache"])?;
        info!(free_bytes = free, ?pressure, "host preflight passed");
        Ok(())
    }

    pub(super) fn cleanup(&self) -> Result<()> {
        let initial = self.free_bytes()?;
        // The worst single volume, not the sum: adding a second volume's bytes
        // to the first and comparing that against thresholds calibrated for the
        // first reads every machine with a guest volume as full, and the branch
        // it reaches for throws away the compiler caches that were never the
        // problem. `preflight` already decides this way.
        let (pressure, volume) = self.worst_pressure()?;
        info!(free_bytes = initial, ?pressure, volume = %volume.display(), "cleanup started");

        self.prune_host_trees("workspaces/tmp", Self::DAY)?;
        self.prune_host_trees("workspaces/builds", Self::DAY)?;
        self.prune_build_trees("workspaces/gitlab", Self::DAY)?;
        self.prune_host_trees("vm/overlays", Self::DAY)?;
        self.prune_host_trees("vm/android/avd", Self::DAY)?;
        self.prune_host_files("logs", 14 * Self::DAY)?;
        self.rotate_logs()?;
        self.prune_retired_caches(7 * Self::DAY)?;

        match pressure {
            Pressure::Soft => {
                self.prune_host_trees("cache/quarantine", 7 * Self::DAY)?;
                self.prune_host_trees("cache/review", 30 * Self::DAY)?;
                self.prune_host_trees("cache/bootstrap/quarantine", 7 * Self::DAY)?;
                self.prune_host_trees("cache/bootstrap/review", 30 * Self::DAY)?;
                self.prune_docker_cache("720h");
            }
            Pressure::Aggressive | Pressure::Reject => {
                self.prune_host_trees("cache/quarantine", Duration::ZERO)?;
                self.prune_host_trees("cache/review", Duration::ZERO)?;
                self.prune_host_trees("cache/bootstrap/quarantine", Duration::ZERO)?;
                self.prune_host_trees("cache/bootstrap/review", Duration::ZERO)?;
                self.prune_host_trees("cache/trusted", 7 * Self::DAY)?;
                self.prune_host_trees("cache/bootstrap/trusted", 7 * Self::DAY)?;
                self.prune_host_trees("vm/tart/cache", 7 * Self::DAY)?;
                self.prune_docker_cache("168h");
            }
            Pressure::Normal => {}
        }

        // Unconditional, and after the pruning above, because the guest frees
        // blocks on its own schedule and holds them until asked. Its root
        // filesystem is mounted `discard` and stays at a gigabyte, but the data
        // disk carrying `/var/lib/docker` is not, so every layer Docker deletes
        // stays allocated in a file this volume pays for. One trim on a machine
        // that had drifted to 44 GB free returned 63 of them in seconds — too
        // cheap to hold back for a threshold, and holding it back is what let
        // the drift reach refusal five times.
        self.trim_linux_guest();

        let target_dirs = persistent_target_dirs(&self.build_root.join("workspaces/gitlab"))?;
        build_cache::enforce_budget(&target_dirs, self.config.host.build_cache_budget_bytes()?)?;

        let (mut final_pressure, _) = self.worst_pressure()?;
        if final_pressure == Pressure::Reject {
            // The guests are where the space is, and the caches are what the
            // steps above can reach — so taking the caches first pays for the
            // guests with the compiler output the machine exists to keep warm.
            // The macOS one gives its space back only when it is thrown away:
            // 38 gibibytes in a recycle against three and a half for
            // everything else together. The Linux one has already been
            // trimmed, so what is left in it is what Docker still holds and
            // the prune window would not take — layers younger than a week.
            // Only recycling reaches those, and it costs the job in flight and
            // a cold image build, which is a trade worth making once jobs are
            // being refused anyway — this branch.
            self.recycle_linux_guest();
            self.recycle_macos_guest();
            final_pressure = self.worst_pressure()?.0;
        }
        if final_pressure == Pressure::Reject {
            self.prune_host_trees("cache/trusted", Duration::ZERO)?;
            self.prune_host_trees("cache/bootstrap/trusted", Duration::ZERO)?;
            self.prune_retired_caches(Duration::ZERO)?;
            final_pressure = self.worst_pressure()?.0;
        }
        info!(
            free_bytes = self.free_bytes()?,
            ?final_pressure,
            "cleanup completed"
        );
        if final_pressure == Pressure::Reject {
            bail!("CI volume remains above the new-job threshold after cleanup");
        }
        Ok(())
    }

    pub(super) fn health(&self) -> Result<()> {
        let free = self.free_bytes()?;
        let volumes: Vec<VolumeHealth> = self
            .volumes()?
            .into_iter()
            .map(|volume| VolumeHealth {
                path: volume.path.display().to_string(),
                free_bytes: volume.available,
                total_bytes: volume.total,
                pressure: self.pressure_of(&volume),
            })
            .collect();
        let (pressure, worst) = self.worst_pressure()?;
        let agents = self.agent_states();
        let down: Vec<&str> = agents
            .iter()
            .filter(|(_, state)| ![Agents::RUNNING, Agents::ABSENT].contains(*state))
            .map(|(name, _)| *name)
            .collect();
        serde_json::to_writer(
            io::stdout().lock(),
            &Health {
                free_bytes: free,
                pressure,
                volumes,
                agents,
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
        if !down.is_empty() {
            bail!("CI agents are not running: {}", down.join(", "));
        }
        Ok(())
    }

    /// What launchd says about each agent work depends on.
    ///
    /// On a host with no launchd there is nothing to say, and this must not
    /// invent a fault: the Linux executor runs the same command.
    fn agent_states(&self) -> BTreeMap<&'static str, &'static str> {
        if !Path::new(Agents::LAUNCHCTL).is_file() {
            return Agents::ALWAYS_ON
                .iter()
                .map(|name| (*name, Agents::ABSENT))
                .collect();
        }
        let listing = self
            .process
            .capture(Agents::LAUNCHCTL, &["list"], "launchd agent listing")
            .unwrap_or_default();
        agent_states_from(&listing)
    }

    /// What is left on the volume with the least to spare — the one the pressure
    /// verdict comes from, since every volume is judged against the same floors.
    ///
    /// Summing bytes spent across volumes measured neither: on a shared APFS
    /// container a volume's used space counts what its neighbours hold, so the
    /// total ran past the quota it was being compared with. This host reported
    /// 470 GB spent on a 300 GB quota while it had 44 GB free.
    fn free_bytes(&self) -> Result<u64> {
        self.volumes()?
            .into_iter()
            .map(|volume| volume.available)
            .min()
            .context("no CI volume to measure")
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
        let mut volumes = vec![Volume::read(&self.host_root)?];
        if self.build_root != self.host_root
            && let Ok(build_root) = self.build_root.canonicalize()
            && !build_root.starts_with(&self.host_root)
        {
            volumes.push(Volume::read(&build_root)?);
        }
        let guests = self.host_root.join("vm");
        if guests.is_dir()
            && let Ok(guests) = guests.canonicalize()
            && !guests.starts_with(&self.host_root)
            && !volumes
                .iter()
                .any(|volume| guests.starts_with(&volume.path))
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
        let mut worst = (Pressure::Normal, self.host_root.clone());
        for volume in volumes {
            let pressure = self.pressure_of(&volume);
            if pressure > worst.0 {
                worst = (pressure, volume.path);
            }
        }
        Ok(worst)
    }

    /// Pressure is what is left, not what was spent.
    ///
    /// The thresholds are written as bytes used against `quota_bytes`, and this
    /// reads them as the free space each one intends to keep: a volume at the
    /// reject threshold is one with `quota - reject` bytes to spare. On an APFS
    /// container the volume shares, the two are not the same question. This one
    /// measured 279 GB with 170 used — never within 100 GB of a 285 GB reject
    /// threshold, so cleanup stayed `Normal` and never recycled the guest —
    /// while jobs were already being refused for having 10 GB free where the
    /// preflight wants 15.
    ///
    /// Read this way the ladder and the refusal agree by construction:
    /// `quota - reject` is exactly the free space a job is required to find.
    fn pressure_of(&self, volume: &Volume) -> Pressure {
        pressure_for(
            volume.available,
            self.floor(self.config.host.soft_cleanup_bytes),
            self.floor(self.config.host.aggressive_cleanup_bytes),
            self.floor(self.config.host.reject_bytes),
        )
    }

    /// The free space a used-bytes threshold was asking for.
    fn floor(&self, threshold: u64) -> u64 {
        self.config.host.quota_bytes.saturating_sub(threshold)
    }

    /// Cache namespaces nothing writes to any more, once they have gone quiet
    /// for a week.
    fn prune_retired_caches(&self, age: Duration) -> Result<()> {
        let directory = self.host_root.join("cache");
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

    fn prune_host_trees(&self, relative: &str, age: Duration) -> Result<()> {
        self.prune_old_trees(&self.host_root, relative, age)
    }

    fn prune_build_trees(&self, relative: &str, age: Duration) -> Result<()> {
        self.prune_old_trees(&self.build_root, relative, age)
    }

    fn prune_old_trees(&self, root: &Path, relative: &str, age: Duration) -> Result<()> {
        let directory = root.join(relative);
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

    fn prune_host_files(&self, relative: &str, age: Duration) -> Result<()> {
        let directory = self.host_root.join(relative);
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
        let directory = self.host_root.join("logs");
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
        if !self.is_removable(path) {
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
        let home = self.host_root.join("home").join(&self.config.host.ci_user);
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

    /// Hand the guest's freed blocks back to the volume.
    ///
    /// Pruning inside the guest returns nothing here on its own: the data disk
    /// is a sparse file that grows to its high-water mark and keeps every block
    /// it has ever written. Measured on 2026-08-11 it held 77 GB for 25 GB of
    /// images and build cache — fifty gigabytes of blocks the guest had already
    /// released. A discard is what tells the host they are free, and it is the
    /// step between pruning, which frees nothing here, and recycling, which
    /// costs a cold image build.
    fn trim_linux_guest(&self) {
        let colima = self.config.host.brew_tool("colima");
        if !colima.is_file() {
            return;
        }
        let home = self.host_root.join("home").join(&self.config.host.ci_user);
        let mut command = self.process.command(colima);
        command.env("COLIMA_HOME", home.join(".colima")).args([
            "ssh",
            "--profile",
            Self::COLIMA_PROFILE,
            "--",
            "sudo",
            "fstrim",
            "-a",
        ]);
        if let Err(error) = self
            .process
            .run_command(&mut command, "return the Linux guest's freed blocks")
        {
            warn!(%error, "could not trim the Linux guest's disk");
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
        let home = self.host_root.join("home").join(&self.config.host.ci_user);
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

    fn is_removable(&self, target: &Path) -> bool {
        is_removable_under(&self.host_root, target, Self::REMOVABLE_ROOTS)
            || is_removable_under(&self.build_root, target, &["workspaces"])
    }
}

fn is_removable_under(root: &Path, target: &Path, removable_roots: &[&str]) -> bool {
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
    removable_roots
        .iter()
        .any(|allowed| first == std::ffi::OsStr::new(allowed))
        && components.next().is_some()
        && relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// Floors are free-space limits, so a smaller number is a tighter one: at or
/// below `reject` the volume refuses work.
pub(super) fn pressure_for(available: u64, soft: u64, aggressive: u64, reject: u64) -> Pressure {
    if available <= reject {
        Pressure::Reject
    } else if available <= aggressive {
        Pressure::Aggressive
    } else if available <= soft {
        Pressure::Soft
    } else {
        Pressure::Normal
    }
}

/// `launchctl list` prints `PID  status  label`, and the dash in the PID column
/// is the point: an agent restarted by `KeepAlive` stays loaded while holding no
/// process, which is what a crash loop looks like from outside.
fn agent_states_from(listing: &str) -> BTreeMap<&'static str, &'static str> {
    Agents::ALWAYS_ON
        .iter()
        .map(|name| {
            let label = format!("com.zvuk.kithara-ci.{name}");
            let state = listing
                .lines()
                .find_map(|line| {
                    let mut columns = line.split_whitespace();
                    let pid = columns.next()?;
                    columns.next()?;
                    (columns.next()? == label).then(|| {
                        if pid == "-" {
                            "stopped"
                        } else {
                            Agents::RUNNING
                        }
                    })
                })
                .unwrap_or("not-loaded");
            (*name, state)
        })
        .collect()
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
        // 300 GB quota with 240/270/285 used-byte thresholds keeps 60/30/15
        // free.
        assert_eq!(pressure_for(61, 60, 30, 15), Pressure::Normal);
        assert_eq!(pressure_for(60, 60, 30, 15), Pressure::Soft);
        assert_eq!(pressure_for(30, 60, 30, 15), Pressure::Aggressive);
        assert_eq!(pressure_for(15, 60, 30, 15), Pressure::Reject);
    }

    #[test]
    fn a_volume_smaller_than_the_quota_still_reaches_reject() {
        let directory = tempfile::tempdir().unwrap();
        let cfg = config(directory.path());
        let process = Process::new(directory.path(), BTreeMap::new());
        let storage = HostStorage::for_test(&cfg, &process).unwrap();
        // The volume this replaces a proportional rule for: 279 GB total with
        // 170 used never came within a hundred of a 285 reject threshold, so
        // cleanup stayed `Normal` and never recycled the guest, while jobs were
        // already being refused for free space. Judged by what is left, a
        // volume half the quota's size reaches every step.
        let half = |available| Volume {
            path: PathBuf::from("/elsewhere"),
            total: 150,
            available,
        };

        assert_eq!(storage.pressure_of(&half(61)), Pressure::Normal);
        assert_eq!(storage.pressure_of(&half(60)), Pressure::Soft);
        assert_eq!(storage.pressure_of(&half(30)), Pressure::Aggressive);
        assert_eq!(storage.pressure_of(&half(15)), Pressure::Reject);
    }

    /// A tempdir has more than the sixty bytes this config calls a floor, so the
    /// run below is `Normal` — the pressure that used to skip the trim entirely.
    #[cfg(unix)]
    #[test]
    fn the_guest_is_trimmed_even_with_nothing_under_pressure() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let mut cfg = config(directory.path());
        cfg.host.brew_root = directory.path().join("brew");
        let bin = cfg.host.brew_root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let asked = directory.path().join("asked");
        fs::write(
            bin.join("colima"),
            format!("#!/bin/sh\necho \"$@\" > {}\n", asked.display()),
        )
        .unwrap();
        fs::set_permissions(bin.join("colima"), fs::Permissions::from_mode(0o755)).unwrap();

        let process = Process::new(directory.path(), BTreeMap::new());
        let storage = HostStorage::for_test(&cfg, &process).unwrap();
        assert_eq!(storage.worst_pressure().unwrap().0, Pressure::Normal);
        storage.cleanup().unwrap();

        let arguments = fs::read_to_string(&asked).expect("the guest was never asked for anything");
        assert!(
            arguments.contains("fstrim"),
            "cleanup asked the guest for {arguments} instead of a trim"
        );
    }

    /// Verbatim from the host while the macOS runner was crash-looping.
    const CRASH_LOOP_LISTING: &str = "\
82778\t0\tcom.zvuk.kithara-ci.gitlab-runner
-\t0\tcom.zvuk.kithara-ci.health
-\t1\tcom.zvuk.kithara-ci.macos-runner
-\t0\tcom.zvuk.kithara-ci.cleanup
54543\t0\tcom.zvuk.kithara-ci.colima
";

    #[test]
    fn an_agent_restarting_into_nothing_reads_as_stopped() {
        assert_eq!(
            agent_states_from(CRASH_LOOP_LISTING).get("macos-runner"),
            Some(&"stopped")
        );
    }

    #[test]
    fn the_agents_still_holding_a_process_read_as_running() {
        let states = agent_states_from(CRASH_LOOP_LISTING);
        assert_eq!(states.get("gitlab-runner"), Some(&"running"));
        assert_eq!(states.get("colima"), Some(&"running"));
    }

    /// An agent nobody ever loaded is as unable to take work as one that keeps
    /// dying, and reads differently so an operator knows which to fix.
    #[test]
    fn an_agent_missing_from_the_listing_reads_as_not_loaded() {
        assert_eq!(
            agent_states_from("82778\t0\tcom.zvuk.kithara-ci.gitlab-runner\n").get("macos-runner"),
            Some(&"not-loaded")
        );
    }

    #[test]
    fn a_full_volume_is_not_hidden_by_a_roomy_one() {
        assert!(Pressure::Reject > Pressure::Normal);
        assert!(Pressure::Aggressive > Pressure::Soft);
    }

    #[test]
    fn a_distinct_checkout_volume_is_monitored() {
        let directory = tempfile::tempdir().unwrap();
        let host_root = directory.path().join("host");
        let build_root = directory.path().join("builds");
        fs::create_dir_all(&host_root).unwrap();
        fs::create_dir_all(&build_root).unwrap();
        let mut cfg = config(&host_root);
        cfg.host.build_root = Some(build_root.clone());
        let process = Process::new(directory.path(), BTreeMap::new());
        let storage = HostStorage::for_test(&cfg, &process).unwrap();

        let volumes = storage.volumes().unwrap();

        assert!(volumes.iter().any(|volume| volume.path == host_root));
        assert!(
            volumes
                .iter()
                .any(|volume| volume.path == build_root.canonicalize().unwrap())
        );
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
        let storage = HostStorage::for_test(&cfg, &process).unwrap();

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
        let storage = HostStorage::for_test(&cfg, &process).unwrap();

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
        let storage = HostStorage::for_test(&cfg, &process).unwrap();
        let safe = directory.path().join("workspaces/tmp/old");
        fs::create_dir_all(&safe).unwrap();
        storage.remove_path(&safe).unwrap();
        assert!(!safe.exists());

        let outside = directory.path().parent().unwrap().join("outside");
        assert!(storage.remove_path(&outside).is_err());
    }

    #[test]
    fn cleanup_allows_only_workspace_descendants_on_the_checkout_root() {
        let directory = tempfile::tempdir().unwrap();
        let host_root = directory.path().join("host");
        let build_root = directory.path().join("builds");
        fs::create_dir_all(&host_root).unwrap();
        fs::create_dir_all(&build_root).unwrap();
        let mut cfg = config(&host_root);
        cfg.host.build_root = Some(build_root.clone());
        let process = Process::new(directory.path(), BTreeMap::new());
        let storage = HostStorage::for_test(&cfg, &process).unwrap();

        let workspace = build_root.join("workspaces/gitlab/old");
        fs::create_dir_all(&workspace).unwrap();
        storage.remove_path(&workspace).unwrap();
        assert!(!workspace.exists());

        assert!(storage.remove_path(&build_root).is_err());
        let cache = build_root.join("cache/old");
        fs::create_dir_all(&cache).unwrap();
        assert!(storage.remove_path(&cache).is_err());
    }

    #[test]
    fn persistent_targets_are_discovered_under_the_checkout_root() {
        let directory = tempfile::tempdir().unwrap();
        let host_root = directory.path().join("host");
        let build_root = directory.path().join("builds");
        let checkout = build_root.join("workspaces/gitlab/project");
        fs::create_dir_all(checkout.join("target/debug")).unwrap();
        fs::write(checkout.join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::create_dir_all(host_root.join("workspaces/gitlab/stale/target/debug")).unwrap();
        fs::write(
            host_root.join("workspaces/gitlab/stale/Cargo.toml"),
            "[workspace]\n",
        )
        .unwrap();

        let targets = persistent_target_dirs(&build_root.join("workspaces/gitlab")).unwrap();

        assert_eq!(targets, [checkout.join("target")]);
    }

    #[test]
    fn gitlab_workspace_pruning_uses_the_checkout_root() {
        let directory = tempfile::tempdir().unwrap();
        let host_root = directory.path().join("host");
        let build_root = directory.path().join("builds");
        fs::create_dir_all(&host_root).unwrap();
        fs::create_dir_all(&build_root).unwrap();
        let mut cfg = config(&host_root);
        cfg.host.build_root = Some(build_root.clone());
        let process = Process::new(directory.path(), BTreeMap::new());
        let storage = HostStorage::for_test(&cfg, &process).unwrap();
        let selected = build_root.join("workspaces/gitlab/old");
        let stale = host_root.join("workspaces/gitlab/old");
        fs::create_dir_all(&selected).unwrap();
        fs::create_dir_all(&stale).unwrap();

        storage
            .prune_build_trees("workspaces/gitlab", Duration::ZERO)
            .unwrap();

        assert!(!selected.exists());
        assert!(stale.exists());
    }

    #[test]
    fn active_marker_pins_a_workspace() {
        let directory = tempfile::tempdir().unwrap();
        for name in HostStorage::REMOVABLE_ROOTS {
            fs::create_dir_all(directory.path().join(name)).unwrap();
        }
        let cfg = config(directory.path());
        let process = Process::new(directory.path(), BTreeMap::new());
        let storage = HostStorage::for_test(&cfg, &process).unwrap();
        let workspace = directory.path().join("workspaces/tmp/current");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join(".kithara-ci-active"), b"").unwrap();
        assert!(storage.active(&workspace));
    }
}
