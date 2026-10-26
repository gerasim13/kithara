use std::{
    collections::BTreeMap,
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path as FsPath, PathBuf},
    thread,
    time::Instant,
};

use anyhow::{Context, Result, bail};
use kithara_devtools::{Ctx, lease, lock::FileLock};
use serde::{Deserialize, Serialize};
use tracing::warn;

use super::{
    build_cache,
    build_dir::{LaneTarget, Target},
    cache::missing_defaults,
    config::{CiConfig, validate_verdict_paths},
    kache::{KacheJob, LocalStore},
    run::CacheGroup,
};
use crate::{consts, job::is_gitlab};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CacheTrust {
    Quarantine,
    Review,
    Trusted,
}

impl CacheTrust {
    /// Every namespace `prepare` can hand a lane, so a sweep over the scratch
    /// root covers everything that can appear in it.
    pub(super) const ALL: [Self; 3] = [Self::Quarantine, Self::Review, Self::Trusted];

    pub(super) fn from_environment() -> Result<Self> {
        Self::read(&process_var)
    }

    /// The trust `KITHARA_CACHE_TRUST` names; `review` when it names none.
    pub(super) fn read(var: &dyn Fn(&str) -> Option<OsString>) -> Result<Self> {
        let Some(value) = var("KITHARA_CACHE_TRUST") else {
            return Ok(Self::Review);
        };
        match value.to_str() {
            Some("quarantine") => Ok(Self::Quarantine),
            Some("review") => Ok(Self::Review),
            Some("trusted") => Ok(Self::Trusted),
            _ => bail!(
                "unsupported KITHARA_CACHE_TRUST value: {}",
                value.to_string_lossy()
            ),
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Quarantine => "quarantine",
            Self::Review => "review",
            Self::Trusted => "trusted",
        }
    }
}

fn parse_decimal_id(name: &str, value: &str) -> Result<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("{name} must be a decimal integer");
    }
    value
        .parse()
        .with_context(|| format!("{name} must be a decimal integer"))
}

fn lease_owner(job_id: Option<&str>, pid: u32) -> Result<String> {
    if let Some(job_id) = job_id {
        Ok(format!("job-{}", parse_decimal_id("CI_JOB_ID", job_id)?))
    } else {
        Ok(format!("pid-{pid}"))
    }
}

/// The name this job holds what is its own under.
pub(crate) fn job_owner() -> Result<String> {
    let job_id = if is_gitlab() {
        Some(env::var("CI_JOB_ID").context("CI_JOB_ID must identify the GitLab job")?)
    } else {
        None
    };
    lease_owner(job_id.as_deref(), std::process::id())
}

fn cache_lease(cache_root: &FsPath) -> Result<PathBuf> {
    Ok(cache_root.join(".kithara-ci-leases").join(job_owner()?))
}

/// Where the compiler cache keeps its local store, and how large it may grow.
struct SccachePaths {
    directory: PathBuf,
    cache_size: String,
}

impl SccachePaths {
    /// The cache a job compiles through, or none: a Windows target builds
    /// without it, and so does a cache group that does not use it.
    fn for_job(
        target_is_windows: bool,
        cache_root: &FsPath,
        config: &CiConfig,
        cache_group: CacheGroup,
    ) -> Option<Self> {
        (!target_is_windows && cache_group.uses_sccache()).then(|| Self {
            directory: cache_root.join("sccache"),
            cache_size: config.host.sccache_size.clone(),
        })
    }
}

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct CiEnvironment {
    #[field(get, vis = "pub(crate)")]
    shared_root: PathBuf,
    pub(crate) cache_root: PathBuf,
    pub(crate) swiftpm_cache: PathBuf,
    pub(crate) temp: PathBuf,
    leases: [PathBuf; 2],
    /// The job's own kache, its directory removed when the job ends.
    kache: Option<KacheJob>,
    /// Held for the life of the job so a reclaim — this job's own or a sibling
    /// job's — leaves the directory this one builds into alone. The ceiling
    /// still charges its bytes; the claim only says they cannot be taken back.
    _target: Target,
    vars: BTreeMap<OsString, OsString>,
    _ownership: Vec<FileLock>,
}

impl CiEnvironment {
    pub(crate) fn prepare(
        ctx: &Ctx,
        config: &CiConfig,
        cache_group: CacheGroup,
        lane: LaneTarget<'_>,
    ) -> Result<Self> {
        config.validate()?;
        raise_open_file_limit()?;
        let project_root =
            env::var_os("CI_PROJECT_DIR").map_or_else(|| ctx.root.clone(), PathBuf::from);
        let home = env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .context("HOME or USERPROFILE must be set")?;
        let shared_root = select_shared_root(config, cache_group, &home)?;

        let trust = CacheTrust::from_environment()?;
        let platform = format!("{}-{}", env::consts::OS, env::consts::ARCH);
        let namespace = shared_root.join(trust.as_str());
        let cache_root = namespace.join(&platform);
        let fixture_cache = namespace.join("fixtures");
        let scratch = scratch_root();
        let temp = scratch.join(trust.as_str());
        let ownership = claim_namespaces(&[
            (&shared_root, &namespace),
            (&shared_root, &cache_root),
            (&shared_root, &fixture_cache),
            (&scratch, &temp),
        ])?;
        fs::create_dir_all(&shared_root)
            .with_context(|| format!("creating CI cache root {}", shared_root.display()))?;
        // Entered before anything is reclaimed, including by this job itself:
        // its lease keeps every reclaim away from the directory it builds in.
        let target = enter_target(&project_root, &shared_root, config, lane)?;

        let sccache = SccachePaths::for_job(cfg!(windows), &cache_root, config, cache_group);
        let gradle_home = cache_root.join("gradle");
        let leases = [cache_lease(&cache_root)?, cache_lease(&fixture_cache)?];
        let npm_cache = cache_root.join("npm");
        let swiftpm_cache = cache_root.join("swiftpm");

        for directory in [
            &cache_root,
            &gradle_home,
            &fixture_cache,
            &npm_cache,
            &swiftpm_cache,
            &temp,
        ] {
            fs::create_dir_all(directory)
                .with_context(|| format!("creating CI directory {}", directory.display()))?;
        }
        if let Some(sccache) = &sccache {
            fs::create_dir_all(&sccache.directory).with_context(|| {
                format!("creating CI directory {}", sccache.directory.display())
            })?;
        }
        for lease in &leases {
            let lease_root = lease
                .parent()
                .context("CI cache lease must have a parent directory")?;
            fs::create_dir_all(lease_root)
                .with_context(|| format!("creating CI lease directory {}", lease_root.display()))?;
        }
        let mut vars = BTreeMap::new();
        set_path(&mut vars, &home, config)?;
        insert(&mut vars, "CARGO_INCREMENTAL", "0");
        if let Some(dir) = target.cargo_dir() {
            insert(&mut vars, "CARGO_TARGET_DIR", dir);
        }
        // Same reasoning as the justfile's: the system git fetches a large
        // git history far faster, but it fetches with the machine's
        // credentials, and a Linux container has none for the challenge
        // GitHub answers its anonymous request with. Cargo's own client asks
        // anonymously, so it is what the fleet without credentials uses.
        insert(
            &mut vars,
            "CARGO_NET_GIT_FETCH_WITH_CLI",
            if cfg!(target_os = "macos") {
                "true"
            } else {
                "false"
            },
        );
        // Same statement as the GitHub fleet's container: a Linux job links
        // with `lld`. The lane executor is the other way a job reaches this
        // machine, and a linker chosen for only one of them is a measurement
        // that does not carry between them.
        if cfg!(target_os = "linux") {
            for (name, value) in consts::LINUX_LINKER_ENV {
                insert(&mut vars, name, value);
            }
        }
        insert(&mut vars, "GRADLE_USER_HOME", gradle_home);
        // Beside the fixtures, for the reason the Linux fleet keeps them there:
        // a model fetched into a job's own temp directory is newer than the
        // build that embedded it, and Cargo rebuilds everything above it.
        insert(
            &mut vars,
            "KITHARA_BEAT_MODEL_CACHE",
            fixture_cache.join("beat-models"),
        );
        insert(&mut vars, "KITHARA_FIXTURE_CACHE", fixture_cache);
        insert(
            &mut vars,
            "KITHARA_NIGHTLY_TOOLCHAIN",
            &config.pins.nightly_toolchain,
        );
        insert(&mut vars, "npm_config_cache", npm_cache);
        insert(
            &mut vars,
            "RUSTUP_HOME",
            env::var_os("RUSTUP_HOME").unwrap_or_else(|| home.join(".rustup").into_os_string()),
        );
        let kache = match &sccache {
            Some(sccache) => {
                insert_sccache_environment(
                    &mut vars,
                    sccache,
                    &project_root,
                    ctx.config.tools.program("sccache"),
                );
                let kache = KacheJob::prepare(
                    temp.join("kache").join(job_owner()?),
                    Some(LocalStore {
                        directory: &cache_root.join("kache"),
                        max_size: &sccache.cache_size,
                    }),
                )?;
                for (name, value) in kache.environment(&ctx.config.tools) {
                    insert(&mut vars, name, value);
                }
                Some(kache)
            }
            None => None,
        };
        insert(&mut vars, "SWIFTPM_CACHE_PATH", &swiftpm_cache);
        insert(&mut vars, "TMPDIR", &temp);
        insert(
            &mut vars,
            "WASM_SLIM_TOOLCHAIN",
            &config.pins.nightly_toolchain,
        );
        if cfg!(windows) {
            insert(&mut vars, "TEMP", &temp);
            insert(&mut vars, "TMP", &temp);
        }

        if cfg!(target_os = "macos") {
            insert_android_environment(&mut vars, config);
        }

        for lease in &leases {
            fs::write(lease, format!("pid={}\n", std::process::id()))
                .with_context(|| format!("creating CI cache lease {}", lease.display()))?;
        }

        Ok(Self {
            shared_root,
            cache_root,
            swiftpm_cache,
            temp,
            leases,
            kache,
            _target: target,
            vars,
            _ownership: ownership,
        })
    }

    pub(crate) fn vars(&self) -> BTreeMap<OsString, OsString> {
        self.vars.clone()
    }

    /// The kache the job compiles through, when it compiles through one.
    pub(crate) const fn kache(&self) -> Option<&KacheJob> {
        self.kache.as_ref()
    }
}

impl Drop for CiEnvironment {
    fn drop(&mut self) {
        for lease in &self.leases {
            let _ = fs::remove_file(lease);
        }
    }
}

/// Scratch space answers to three constraints at once. It sits outside the
/// checkout, or tools that walk the working tree — the architecture reporter,
/// for one — trip over the temporary copies they just created. It stays short,
/// because macOS caps Unix socket paths at `SUN_LEN` and the suite binds
/// sockets here. And it lives on local storage: the macOS guest reaches the
/// shared cache over virtiofs, which cannot bind a socket at all.
pub(crate) fn scratch_root() -> PathBuf {
    PathBuf::from("/tmp/kithara-ci")
}

#[cfg(unix)]
fn raise_open_file_limit() -> Result<()> {
    use nix::sys::resource::{Resource, getrlimit, setrlimit};

    let (soft, hard) =
        getrlimit(Resource::RLIMIT_NOFILE).context("reading the file descriptor limit")?;
    let target = hard.min(consts::OPEN_FILES);
    if soft >= target {
        return Ok(());
    }
    setrlimit(Resource::RLIMIT_NOFILE, target, hard)
        .context("raising the file descriptor limit")?;
    Ok(())
}

/// Windows hands out handles from a pool and has no per-process ceiling to
/// lift, so the suite already gets the budget the Unix executors have to ask
/// for.
#[cfg(not(unix))]
fn raise_open_file_limit() -> Result<()> {
    Ok(())
}

fn shared_root(config: &CiConfig, cache_group: CacheGroup) -> PathBuf {
    if let Some(root) = env::var_os("KITHARA_CI_CACHE_ROOT") {
        return PathBuf::from(root);
    }
    match cache_group {
        CacheGroup::Macos => config.host.cache_root_macos.clone(),
        CacheGroup::Linux => config.host.cache_root_linux.clone(),
        CacheGroup::Windows => config.host.cache_root_windows.clone(),
        CacheGroup::Host => config.host.host_root.join("cache"),
    }
}

fn select_shared_root(
    config: &CiConfig,
    cache_group: CacheGroup,
    home: &FsPath,
) -> Result<PathBuf> {
    let configured = shared_root(config, cache_group);
    let root = if configured.is_dir() {
        configured
    } else if is_ci() {
        bail!("shared CI cache is not mounted at {}", configured.display());
    } else {
        home.join(".cache/kithara-ci")
    };
    Ok(root)
}

/// The nonswept owner of namespace locks shared by jobs and host reclamation.
pub(crate) fn namespace_lock_root(tree: &FsPath) -> Result<PathBuf> {
    Ok(tree
        .parent()
        .context("cache tree must have a parent")?
        .join(".kithara-job-leases"))
}

/// A resource key includes the tree name so sibling cache trees never share ownership.
pub(crate) fn namespace_key(tree: &FsPath, path: &FsPath) -> Result<PathBuf> {
    let relative = path
        .strip_prefix(tree)
        .context("namespace must be inside its cache tree")?;
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
    {
        bail!("namespace must be a normal relative path below its cache tree");
    }
    Ok(FsPath::new(tree.file_name().context("cache tree must have a name")?).join(relative))
}

fn enter_target(
    project_root: &FsPath,
    shared_root: &FsPath,
    config: &CiConfig,
    lane: LaneTarget<'_>,
) -> Result<Target> {
    if cfg!(target_os = "macos") && is_ci() {
        config.host.validate_verdict_layout(&[])?;
    }
    let mut stores = config.host.verdict_stores().to_vec();
    stores.push(shared_root.join("verdict"));
    let target = Target::enter(project_root, lane, &process_var, &stores)?;
    if is_ci() {
        let volume = match &target {
            Target::Alias { build, .. } => build.path(),
            Target::Named(_) => project_root,
        };
        ensure_room_for_a_job(config, volume, &stores)?;
    }
    Ok(target)
}

fn claim_namespaces(paths: &[(&FsPath, &FsPath)]) -> Result<Vec<FileLock>> {
    let mut ownership = Vec::new();
    if cfg!(target_os = "macos") {
        for &(tree, path) in paths {
            ownership.push(lease::claim(
                &namespace_lock_root(tree)?,
                &namespace_key(tree, path)?,
            )?);
        }
    }
    Ok(ownership)
}

fn is_ci() -> bool {
    ci_in(&process_var)
}

/// Whether the environment `var` reads is a CI job's.
pub(crate) fn ci_in(var: &dyn Fn(&str) -> Option<OsString>) -> bool {
    var("CI").is_some_and(|value| !value.is_empty())
}

/// The process environment, in the shape the readers of an environment take.
pub(super) fn process_var(name: &str) -> Option<OsString> {
    env::var_os(name)
}

/// Refuse a job only once there is nothing left to reclaim and nothing left to
/// wait for.
///
/// The gate and the periodic cleanup never spoke: cleanup ran on a timer and
/// the job arrived when it arrived, so whether a job started came down to how
/// long ago the timer fired. A host under a build loses gigabytes a minute,
/// enough to spend a whole pass's worth of reclaimed space before the next one,
/// and the job landing in that window was refused while tens of gigabytes of
/// evictable compiler cache sat beside it. Growing the disk only moves the
/// window; asking for the space back closes it.
///
/// Asking once does not, because what a reclaim cannot touch is not lost — it
/// is held. A checkout an active job leases is skipped by design, so a host
/// whose slots are all compiling offers no candidate at all, and the refusal
/// reads as "no room" when it means "not yet". Six merge requests were refused
/// this way in one afternoon while the branches beside them went green on the
/// same base; one was 53 MB short, and every one passed on a manual retry
/// against unchanged code. So the gate re-asks until the room appears or the
/// profile's wait runs out, and the retry it was asking a human for costs a
/// poll instead of a whole job.
///
/// The room asked for is on `volume`, the one the job builds on.
fn ensure_room_for_a_job(config: &CiConfig, volume: &FsPath, stores: &[PathBuf]) -> Result<()> {
    let required = config.host.free_bytes_for_a_job();
    if free_bytes(volume)? >= required {
        return Ok(());
    }
    let workspaces = gitlab_workspaces(config.host.build_root());
    let deadline = Instant::now() + config.host.job_room_wait();
    loop {
        let free = free_bytes(volume)?;
        let reclaimed_from = reclaim_build_caches(config, free, required, stores)?;
        let free = free_bytes(volume)?;
        if free >= required {
            return Ok(());
        }
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            bail!(
                "{}",
                refusal(volume, free, required, &workspaces, reclaimed_from)
            );
        };
        warn!(
            free_bytes = free,
            required_bytes = required,
            shortfall_bytes = required.saturating_sub(free),
            seconds_left = left.as_secs(),
            "waiting for the CI volume to give the job room"
        );
        thread::sleep(consts::JOB_ROOM_POLL.min(left));
    }
}

/// The checkouts whose build caches the budget owns.
fn gitlab_workspaces(build_root: &FsPath) -> PathBuf {
    build_root.join("workspaces/gitlab")
}

/// Why the job is refused, in the terms of what the reclaim could act on.
///
/// Saying "after reclaiming build caches" when there was nothing to reclaim
/// pointed every reading of this refusal at the compiler cache, which was
/// neither holding the space nor able to give any back: a workspace a live job
/// leases is skipped, so on a host whose only checkout is the running one the
/// reclaim has no candidate at all and the sentence described work that never
/// happened. What an operator needs at that point is the opposite — that the
/// space is held somewhere this gate does not look.
fn refusal(
    volume: &FsPath,
    free: u64,
    required: u64,
    workspaces: &FsPath,
    reclaimed_from: usize,
) -> String {
    if reclaimed_from == 0 {
        return format!(
            "the volume holding {} has {free} bytes free and a job needs {required}; no \
             reclaimable build cache sits under {}, so the space is held by live work or by \
             trees the build-cache budget does not own",
            volume.display(),
            workspaces.display()
        );
    }
    format!(
        "the volume holding {} has {free} bytes free after reclaiming from {reclaimed_from} \
         build cache(s); a job needs {required} bytes",
        volume.display()
    )
}

/// Return what this host accumulated for itself, before deciding there is no
/// room for the job.
///
/// The gate and the periodic cleanup never spoke: cleanup ran on a timer and
/// the job arrived when it arrived, so whether a job started depended on how
/// long ago the timer fired. A host under a build loses gigabytes a minute,
/// which is enough to spend a whole pass's worth of reclaimed space before the
/// next one — and the job that lands in that window is refused while tens of
/// gigabytes of evictable compiler cache sit beside it. Reclaiming here makes
/// the refusal mean what it says: the space is held by live work, not by
/// leftovers.
///
/// What is reclaimed is the shortfall, not the host's ceiling. The ceiling is
/// the hourly pass's question and it answers "nothing to do" whenever the caches
/// happen to sit under it — which is exactly the state a refused job finds
/// itself in, since a full volume is rarely full of build caches alone.
///
/// Failing to reclaim is not itself a refusal — the gate re-reads free space
/// and answers on that.
fn reclaim_build_caches(
    config: &CiConfig,
    free: u64,
    required: u64,
    stores: &[PathBuf],
) -> Result<usize> {
    let workspaces = gitlab_workspaces(config.host.build_root());
    let cache = config.host.host_root.join("cache");
    let targets = build_cache::budget_roots(&workspaces, &cache)?;
    if cfg!(target_os = "macos") {
        config.host.validate_verdict_layout(&targets)?;
        validate_verdict_paths(&targets, stores)?;
    }
    if targets.is_empty() {
        warn!(
            free_bytes = free,
            required_bytes = required,
            root = %workspaces.display(),
            "no reclaimable build cache to free before refusing the job"
        );
        return Ok(0);
    }
    let shortfall = required.saturating_sub(free);
    warn!(
        free_bytes = free,
        required_bytes = required,
        shortfall_bytes = shortfall,
        targets = targets.len(),
        "reclaiming build caches before refusing the job"
    );
    build_cache::reclaim_at_least(&targets, shortfall)?;
    Ok(targets.len())
}

/// How much room the cache still has. A job reads this through whatever the
/// executor mounted the cache with — a virtiofs share into an ephemeral macOS
/// guest, a bind mount into a container — and those report the filesystem
/// backing the share, which is the host's whole disk rather than the CI volume.
/// Free space survives that translation and still answers the question a job
/// asks; occupancy does not, and comparing the host's disk against a threshold
/// sized for the CI volume rejected every macOS job while the volume was barely
/// half full.
fn free_bytes(path: &FsPath) -> Result<u64> {
    fs4::available_space(path)
        .with_context(|| format!("reading available space for {}", path.display()))
}

fn set_path(
    vars: &mut BTreeMap<OsString, OsString>,
    home: &FsPath,
    config: &CiConfig,
) -> Result<()> {
    let mut paths = vec![home.join(".cargo/bin")];
    if cfg!(target_os = "macos") {
        paths.extend([
            config.host.host_root.join("toolchains/shared-bin"),
            config.host.android_home.join("cmdline-tools/latest/bin"),
            config.host.android_home.join("emulator"),
            config.host.android_home.join("platform-tools"),
            config.host.brew_root.join("bin"),
        ]);
    }
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    let joined = env::join_paths(paths).context("joining CI PATH")?;
    vars.insert(OsString::from("PATH"), joined);
    Ok(())
}

fn insert(
    vars: &mut BTreeMap<OsString, OsString>,
    name: impl AsRef<OsStr>,
    value: impl Into<OsString>,
) {
    vars.insert(name.as_ref().to_os_string(), value.into());
}

/// sccache takes its configuration from the environment it starts in, so
/// what the host left out of its store's environment is filled in here rather
/// than trusted to every host file. Cargo runs it under the name `wrapper`
/// gives: cc-rs hands a build script's C compiles to the same wrapper only
/// when it is named after a compiler cache it knows.
fn insert_sccache_environment(
    vars: &mut BTreeMap<OsString, OsString>,
    paths: &SccachePaths,
    project_root: &FsPath,
    wrapper: &str,
) {
    insert(vars, "RUSTC_WRAPPER", wrapper);
    insert(vars, "SCCACHE_BASEDIRS", project_root);
    insert(vars, "SCCACHE_CACHE_SIZE", &paths.cache_size);
    insert(vars, "SCCACHE_DIR", &paths.directory);
    insert(vars, "SCCACHE_IDLE_TIMEOUT", consts::SCCACHE_IDLE_TIMEOUT);
    for (name, value) in missing_defaults(|name| env::var(name).ok()) {
        insert(vars, name, value);
    }
}

/// The Android toolchain a mac host carries. Only that fleet builds for the
/// device, and the paths are the host profile's rather than this crate's.
fn insert_android_environment(vars: &mut BTreeMap<OsString, OsString>, config: &CiConfig) {
    let android_user_home = config.host.host_root.join("toolchains/android-user");
    insert(vars, "ANDROID_HOME", &config.host.android_home);
    insert(
        vars,
        "ANDROID_NDK_HOME",
        config
            .host
            .android_home
            .join("ndk")
            .join(&config.pins.android_ndk_version),
    );
    insert(vars, "ANDROID_USER_HOME", &android_user_home);
    insert(vars, "ANDROID_AVD_HOME", android_user_home.join("avd"));
    let java_home = config.host.java_home();
    if java_home.is_dir() {
        insert(vars, "JAVA_HOME", &java_home);
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    #[cfg(unix)]
    use kithara_devtools::common::project::ProjectConfig;
    use kithara_devtools::lease;

    use super::*;
    use crate::ci::config::fixture_directory;

    fn reclaim(root: &FsPath) -> usize {
        let mut config = super::super::config::fixture();
        config.host.host_root = root.to_path_buf();
        config.host.build_root = Some(root.to_path_buf());
        config.host.cache_root_macos = root.join("cache");
        reclaim_build_caches(&config, 0, u64::MAX, &config.host.verdict_stores()).unwrap()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_jobs_can_resolve_provisioned_shared_tools() {
        let config = super::super::config::fixture();
        let mut vars = BTreeMap::new();

        set_path(&mut vars, FsPath::new("/ci-home"), &config).unwrap();

        let paths = env::split_paths(vars.get(OsStr::new("PATH")).unwrap()).collect::<Vec<_>>();
        assert!(
            paths.contains(&config.host.host_root.join("toolchains/shared-bin")),
            "CI PATH omits provisioned shared tools: {paths:?}"
        );
    }

    #[test]
    fn the_windows_cache_group_compiles_without_sccache() {
        let config = super::super::config::fixture();

        let paths =
            SccachePaths::for_job(false, FsPath::new("/cache"), &config, CacheGroup::Windows);

        assert!(paths.is_none());
    }

    #[test]
    fn windows_target_disables_sccache_independently_of_lane() {
        let config = super::super::config::fixture();
        let root = FsPath::new("/cache");

        let paths = SccachePaths::for_job(true, root, &config, CacheGroup::Macos);

        assert!(paths.is_none());
    }

    #[test]
    fn lease_names_use_the_job_or_local_process() {
        assert_eq!(lease_owner(Some("29"), 41).unwrap(), "job-29");
        assert_eq!(lease_owner(None, 41).unwrap(), "pid-41");
        assert!(lease_owner(Some("../29"), 41).is_err());
    }

    #[test]
    fn a_job_compiles_through_one_cache_at_the_profiles_budget() {
        let config = super::super::config::fixture();
        let root = FsPath::new("/cache/review/macos-aarch64");

        let paths = SccachePaths::for_job(false, root, &config, CacheGroup::Macos)
            .expect("a macOS job compiles through the cache");

        assert_eq!(paths.directory, root.join("sccache"));
        assert_eq!(paths.cache_size, config.host.sccache_size);
    }

    /// A job compiles through kache, keeps what it compiled in its trust's
    /// store, shares it through its own scope's bucket, and returns where
    /// kache's configuration is.
    #[cfg(unix)]
    fn compiles_through_kache(
        vars: &BTreeMap<OsString, OsString>,
        ctx: &Ctx,
        config: &CiConfig,
        cache_root: &std::path::Path,
    ) -> PathBuf {
        assert_eq!(
            vars.get(OsStr::new("RUSTC_WRAPPER"))
                .map(OsString::as_os_str),
            Some(OsStr::new(ctx.config.tools.program("kache")))
        );
        assert_eq!(
            vars.get(OsStr::new("KACHE_FALLBACK"))
                .map(OsString::as_os_str),
            Some(OsStr::new(ctx.config.tools.program("sccache")))
        );
        let kache_config = PathBuf::from(
            vars.get(OsStr::new("KACHE_CONFIG"))
                .expect("a job tells kache where its configuration is"),
        );
        let kache: toml::Table =
            toml::from_str(&fs::read_to_string(&kache_config).unwrap()).unwrap();
        assert_eq!(
            kache["cache"]["local_store"].as_str().map(PathBuf::from),
            Some(cache_root.join("kache")),
            "what a job compiled stays for the next job of its trust"
        );
        assert_eq!(
            kache["cache"]["local_max_size"].as_str(),
            Some(config.host.sccache_size.as_str())
        );
        assert!(
            kache["cache"].get("remote").is_none(),
            "the remote is the runner's, named in kache's host layer"
        );
        assert!(
            kache_config.starts_with(scratch_root()),
            "kache's socket is bound on local storage"
        );
        kache_config
    }

    /// A GitLab lane builds in a directory of its own beside the checkout,
    /// behind the alias its executor names: Cargo is told the alias, so every
    /// compilation is keyed on one path whichever lane ran there last, and the
    /// checkout's `target` names the lane's directory for the paths artifacts
    /// are collected from. The job claims nothing else: no compiler-cache
    /// socket or slot, and the Cargo home is the executor's.
    #[cfg(unix)]
    #[test]
    fn a_gitlab_lane_builds_beside_its_checkout_behind_the_alias() {
        if env::var_os(consts::LANE_PREPARED).is_some() {
            let root = PathBuf::from(env::var_os(consts::CACHE_ROOT).unwrap());
            let project = PathBuf::from(env::var_os("CI_PROJECT_DIR").unwrap());
            let alias =
                PathBuf::from(env::var_os("CARGO_TARGET_DIR").unwrap()).join(consts::BUILD_ALIAS);
            let ctx = Ctx::new(project.clone(), ProjectConfig::default());
            let mut config = super::super::config::fixture();
            // The gate reads the volume the test runs on; one byte of room is
            // what this job asks of it.
            config.host.reject_bytes = config.host.quota_bytes - 1;

            let environment = CiEnvironment::prepare(
                &ctx,
                &config,
                CacheGroup::Macos,
                LaneTarget {
                    name: "apple-lint",
                    window: consts::DAY,
                },
            )
            .unwrap();
            let vars = environment.vars();
            let build = fs::read_link(&alias).unwrap();

            assert_eq!(
                vars.get(OsStr::new("CARGO_TARGET_DIR")),
                Some(alias.as_os_str().to_owned()).as_ref(),
                "Cargo is told the alias, the one path every lane's compilations share"
            );
            assert_eq!(build.parent(), alias.parent());
            assert_eq!(
                fs::canonicalize(project.join("target")).unwrap(),
                fs::canonicalize(&build).unwrap(),
                "artifact paths name the checkout's target"
            );
            assert!(
                lease::evict(&build).unwrap().is_none(),
                "the job holds the directory it builds in"
            );
            for name in [
                "CARGO_HOME",
                "SCCACHE_SERVER_UDS",
                "CARGO_UNSTABLE_MTIME_ON_USE",
            ] {
                assert!(
                    !vars.contains_key(OsStr::new(name)),
                    "{name} is the executor's to name, or nobody's"
                );
            }
            let cache_root =
                root.join("review")
                    .join(format!("{}-{}", env::consts::OS, env::consts::ARCH));
            let kache_config = compiles_through_kache(&vars, &ctx, &config, &cache_root);
            assert_eq!(
                vars.get(OsStr::new("SCCACHE_DIR")).map(OsString::as_os_str),
                Some(cache_root.join("sccache").as_os_str())
            );
            assert_eq!(
                vars.get(OsStr::new("SCCACHE_CACHE_SIZE"))
                    .map(OsString::as_os_str),
                Some(OsStr::new(&config.host.sccache_size))
            );
            assert_eq!(
                vars.get(OsStr::new("SCCACHE_IDLE_TIMEOUT"))
                    .map(OsString::as_os_str),
                Some(OsStr::new(consts::SCCACHE_IDLE_TIMEOUT))
            );
            // The host named its store and left the prefix out, as every
            // Linux host file did: the server must still write under it.
            assert_eq!(
                vars.get(OsStr::new("SCCACHE_S3_KEY_PREFIX"))
                    .map(OsString::as_os_str),
                Some(OsStr::new("sccache"))
            );
            assert!(!vars.contains_key(OsStr::new("SCCACHE_REGION")));
            assert_eq!(
                vars.get(OsStr::new("KITHARA_FIXTURE_CACHE"))
                    .map(OsString::as_os_str),
                Some(root.join("review/fixtures").as_os_str())
            );
            let models = vars
                .get(OsStr::new("KITHARA_BEAT_MODEL_CACHE"))
                .expect("a job is told where the beat models live");
            assert!(
                PathBuf::from(models).starts_with(root.join("review")),
                "{models:?} is not in the shared cache of the job's trust"
            );
            let lease = cache_root.join(".kithara-ci-leases/job-29");
            assert!(lease.is_file());
            let fixture_lease = root.join("review/fixtures/.kithara-ci-leases/job-29");
            assert!(
                fixture_lease.is_file(),
                "fixture readers must outlive host cleanup"
            );
            drop(environment);
            assert!(
                !kache_config.exists(),
                "a finished job's daemon directory is gone"
            );
            assert!(!lease.exists());
            assert!(!fixture_lease.exists());
            assert!(
                lease::evict(&build).unwrap().is_some(),
                "a finished job leaves its build to the budget"
            );
            return;
        }

        let directory = fixture_directory().unwrap();
        let project = directory
            .path()
            .join("workspaces/gitlab/runner/0/disrupt/kithara");
        fs::create_dir_all(&project).unwrap();
        let init = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&project)
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .status()
            .unwrap();
        assert!(init.success());
        let root = project.with_file_name(".kithara.target");
        let output = Command::new(env::current_exe().unwrap())
            .arg("a_gitlab_lane_builds_beside_its_checkout_behind_the_alias")
            .arg("--nocapture")
            .env(consts::LANE_PREPARED, "1")
            .env(consts::CACHE_ROOT, directory.path())
            .env("KITHARA_CI_CACHE_ROOT", directory.path())
            .env("KITHARA_CACHE_TRUST", "review")
            .env("SCCACHE_BUCKET", "kithara-review")
            .env("SCCACHE_ENDPOINT", "http://kithara-ci-cache:9000")
            .env("SCCACHE_REGION", "us-east-1")
            .env_remove("SCCACHE_S3_KEY_PREFIX")
            .env("CI", "true")
            .env("GITLAB_CI", "true")
            .env("CI_PROJECT_DIR", &project)
            .env("CARGO_TARGET_DIR", &root)
            .env("CI_RUNNER_ID", "999")
            .env("CI_CONCURRENT_ID", "1")
            .env("CI_JOB_ID", "29")
            .env("CI_JOB_URL", "https://gitlab.example/-/jobs/29")
            .env("HOME", directory.path().join("home"))
            .env_remove("CARGO_HOME")
            // A GitLab executor names no slots directory; a Linux runner the
            // suite itself runs on does.
            .env_remove(consts::BUILD_SLOTS_ENV)
            .env_remove("SCCACHE_SERVER_UDS")
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "child failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_prepare_never_publishes_a_cache_lease() {
        if env::var_os(consts::FAILED_PREPARE).is_some() {
            let root = PathBuf::from(env::var_os(consts::CACHE_ROOT).unwrap());
            let project = root.join("project");
            fs::create_dir_all(&project).unwrap();
            let ctx = Ctx::new(project, ProjectConfig::default());
            let config = super::super::config::fixture();

            let lane = LaneTarget {
                name: "lint",
                window: consts::DAY,
            };
            let Err(error) = CiEnvironment::prepare(&ctx, &config, CacheGroup::Macos, lane) else {
                panic!("prepare unexpectedly succeeded");
            };
            assert!(error.to_string().contains("joining CI PATH"));
            let cache_root =
                root.join("review")
                    .join(format!("{}-{}", env::consts::OS, env::consts::ARCH));
            assert!(!cache_root.join(".kithara-ci-leases/job-30").exists());
            return;
        }

        let directory = fixture_directory().unwrap();
        let output = Command::new(env::current_exe().unwrap())
            .arg("failed_prepare_never_publishes_a_cache_lease")
            .arg("--nocapture")
            .env(consts::FAILED_PREPARE, "1")
            .env(consts::CACHE_ROOT, directory.path())
            .env("KITHARA_CI_CACHE_ROOT", directory.path())
            .env("KITHARA_CACHE_TRUST", "review")
            .env("GITLAB_CI", "true")
            .env("CI_CONCURRENT_ID", "0")
            .env("CI_JOB_ID", "30")
            .env("HOME", directory.path().join("invalid:home"))
            .env_remove("CI")
            .env_remove("CI_PROJECT_DIR")
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "child failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn cache_trust_is_strict() {
        assert_eq!(CacheTrust::Review.as_str(), "review");
        assert_eq!(CacheTrust::Quarantine.as_str(), "quarantine");
        assert_eq!(CacheTrust::Trusted.as_str(), "trusted");
    }

    #[test]
    fn cache_trust_reads_review_when_unset_and_refuses_what_it_does_not_know() {
        let named = |value: &'static str| {
            move |name: &str| -> Option<OsString> {
                (name == "KITHARA_CACHE_TRUST").then(|| OsString::from(value))
            }
        };

        assert_eq!(
            CacheTrust::read(&|_: &str| -> Option<OsString> { None }).unwrap(),
            CacheTrust::Review
        );
        assert_eq!(
            CacheTrust::read(&named("trusted")).unwrap(),
            CacheTrust::Trusted
        );
        let error = CacheTrust::read(&named("public")).unwrap_err();
        assert!(
            error.to_string().contains("public"),
            "the error must name the value: {error}"
        );
    }

    /// A checkout under `root` and the build root beside it, holding build
    /// `id` with one built unit, the alias naming it as the executor leaves
    /// it. Returns the build's directory.
    #[cfg(unix)]
    fn build_beside_a_checkout(root: &FsPath, id: &str) -> PathBuf {
        let checkout = root.join("workspaces/gitlab/runner-a/0/disrupt/kithara");
        fs::create_dir_all(&checkout).unwrap();
        fs::write(checkout.join("Cargo.toml"), "[package]\n").unwrap();
        let build_root = checkout.with_file_name(".kithara.target");
        fs::create_dir_all(&build_root).unwrap();
        let id = format!("{id}-0");
        let slot = fs::File::create(build_cache::lock_beside(&build_root.join(&id))).unwrap();
        let _slot = FileLock::try_exclusive(slot).unwrap();
        let unit = build_root.join(&id).join("debug");
        fs::create_dir_all(&unit).unwrap();
        fs::write(unit.join("artifact"), vec![0_u8; 400_000]).unwrap();
        std::os::unix::fs::symlink(&id, build_root.join(consts::BUILD_ALIAS)).unwrap();
        build_root.join(id)
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn preparation_rejects_verdict_storage_before_slot_garbage_collection() {
        if env::var_os(consts::FAILED_PREPARE).is_some() {
            let root = PathBuf::from(env::var_os(consts::CACHE_ROOT).unwrap());
            let project = PathBuf::from(env::var_os("CI_PROJECT_DIR").unwrap());
            let store = PathBuf::from(env::var_os("EXPECTED_VERDICT_STORE").unwrap());
            let mut config = super::super::config::fixture();
            config.host.host_root = root.clone();
            config.host.build_root = Some(root);
            config.host.cache_root_macos =
                PathBuf::from(env::var_os("EXPECTED_PROFILE_CACHE").unwrap());
            config.host.reject_bytes = config.host.quota_bytes - 1;
            let ctx = Ctx::new(project.clone(), ProjectConfig::default());
            let result = CiEnvironment::prepare(
                &ctx,
                &config,
                CacheGroup::Macos,
                LaneTarget {
                    name: "apple-lint",
                    window: consts::DAY,
                },
            );
            for name in ["journal.json", "journal.json.lock", "journal.json.42.tmp"] {
                assert!(
                    store.join(name).is_file(),
                    "prepare removed verdict file {name}"
                );
            }
            let Err(error) = result else {
                panic!("overlapping storage was accepted");
            };
            assert!(error.to_string().contains("must not overlap"), "{error:#}");
            assert!(!project.join("target").exists());
            return;
        }
        for (scratch, linked) in [
            ("tmp", false),
            ("cargo-timings", false),
            ("tmp", true),
            ("cargo-timings", true),
            ("override", false),
        ] {
            let directory = fixture_directory().unwrap();
            let project = directory
                .path()
                .join("workspaces/gitlab/runner/0/disrupt/kithara");
            fs::create_dir_all(&project).unwrap();
            let target = project.with_file_name(".kithara.target");
            let slot = if linked {
                directory.path().join("physical-slot")
            } else {
                target.join("apple-lint-0")
            };
            let store = slot
                .join(if scratch == "override" {
                    "tmp"
                } else {
                    scratch
                })
                .join("verdict");
            fs::create_dir_all(&store).unwrap();
            if linked {
                fs::create_dir_all(&target).unwrap();
                std::os::unix::fs::symlink(&slot, target.join("apple-lint-0")).unwrap();
            }
            let files = [
                (
                    "journal.json",
                    br#"{"runs":[{"sha":"main","tests":[],"jobs":[]}]}"#.as_slice(),
                ),
                ("journal.json.lock", b"baseline lock".as_slice()),
                ("journal.json.42.tmp", b"publication".as_slice()),
            ];
            let identities = files.map(|(name, bytes)| {
                fs::write(store.join(name), bytes).unwrap();
                same_file::Handle::from_path(store.join(name)).unwrap()
            });
            let output = Command::new(env::current_exe().unwrap())
                .args([
                    "preparation_rejects_verdict_storage_before_slot_garbage_collection",
                    "--nocapture",
                ])
                .env(consts::FAILED_PREPARE, "1")
                .env(consts::CACHE_ROOT, directory.path())
                .env("EXPECTED_VERDICT_STORE", &store)
                .env("CI_PROJECT_DIR", &project)
                .env("CARGO_TARGET_DIR", &target)
                .env("CI", "true")
                .env("KITHARA_CACHE_TRUST", "review")
                .env("KITHARA_CI_CACHE_ROOT", store.parent().unwrap())
                .env(
                    "EXPECTED_PROFILE_CACHE",
                    if scratch == "override" {
                        directory.path().join("configured-cache")
                    } else {
                        store.parent().unwrap().to_path_buf()
                    },
                )
                .env_remove(consts::BUILD_SLOTS_ENV)
                .env_remove("GIT_DIR")
                .env_remove("GIT_INDEX_FILE")
                .env_remove("GIT_WORK_TREE")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{scratch}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            for ((name, bytes), identity) in files.into_iter().zip(identities) {
                assert_eq!(fs::read(store.join(name)).unwrap(), bytes);
                assert_eq!(
                    same_file::Handle::from_path(store.join(name)).unwrap(),
                    identity
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn preparation_preserves_a_peer_verdict_store_under_pressure() {
        if env::var_os(consts::FAILED_PREPARE).is_some() {
            let root = PathBuf::from(env::var_os(consts::CACHE_ROOT).unwrap());
            let project = PathBuf::from(env::var_os("CI_PROJECT_DIR").unwrap());
            let store =
                PathBuf::from(env::var_os("KITHARA_CI_CACHE_ROOT").unwrap()).join("verdict");
            let mut config = super::super::config::fixture();
            config.host.host_root = root.clone();
            config.host.build_root = Some(root.clone());
            config.host.cache_root_macos = root.join("configured-cache");
            config.host.quota_bytes = u64::MAX;
            config.host.job_room_wait_seconds = 1;
            let result = CiEnvironment::prepare(
                &Ctx::new(project, ProjectConfig::default()),
                &config,
                CacheGroup::Macos,
                LaneTarget {
                    name: "apple-lint",
                    window: consts::DAY,
                },
            );
            for name in ["journal.json", "journal.json.lock", "journal.json.42.tmp"] {
                assert!(
                    store.join(name).is_file(),
                    "pressure reclaim removed peer verdict file {name}"
                );
            }
            let Err(error) = result else {
                panic!("overlapping peer storage was accepted");
            };
            assert!(error.to_string().contains("must not overlap"), "{error:#}");
            return;
        }
        let root = fixture_directory().unwrap();
        let peer = build_beside_a_checkout(root.path(), "lint");
        let store = peer.join("shared-cache/verdict");
        fs::create_dir_all(&store).unwrap();
        let project = root
            .path()
            .join("workspaces/gitlab/runner-b/0/disrupt/kithara");
        fs::create_dir_all(&project).unwrap();
        let init = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&project)
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .status()
            .unwrap();
        assert!(init.success());
        let files = [
            (
                "journal.json",
                br#"{"runs":[{"sha":"main","tests":[],"jobs":[]}]}"#.as_slice(),
            ),
            ("journal.json.lock", b"baseline lock".as_slice()),
            ("journal.json.42.tmp", b"publication".as_slice()),
        ];
        let identities = files.map(|(name, bytes)| {
            fs::write(store.join(name), bytes).unwrap();
            same_file::Handle::from_path(store.join(name)).unwrap()
        });
        let output = Command::new(env::current_exe().unwrap())
            .args([
                "preparation_preserves_a_peer_verdict_store_under_pressure",
                "--nocapture",
            ])
            .env(consts::FAILED_PREPARE, "1")
            .env(consts::CACHE_ROOT, root.path())
            .env("CI_PROJECT_DIR", &project)
            .env(
                "CARGO_TARGET_DIR",
                project.with_file_name(".kithara.target"),
            )
            .env("CI", "true")
            .env("KITHARA_CACHE_TRUST", "review")
            .env("KITHARA_CI_CACHE_ROOT", store.parent().unwrap())
            .env_remove(consts::BUILD_SLOTS_ENV)
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        for ((name, bytes), identity) in files.into_iter().zip(identities) {
            assert_eq!(fs::read(store.join(name)).unwrap(), bytes);
            assert_eq!(
                same_file::Handle::from_path(store.join(name)).unwrap(),
                identity
            );
        }
        assert!(peer.join("debug/artifact").is_file());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn job_side_reclamation_refuses_a_verdict_store_inside_a_build() {
        use std::os::unix::fs::MetadataExt;

        for layout in [
            "direct",
            "default-store-link",
            "configured-store-link",
            "case",
        ] {
            let root = fixture_directory().unwrap();
            let build = build_beside_a_checkout(root.path(), "lint");
            let mut config = super::super::config::fixture();
            config.host.host_root = root.path().to_path_buf();
            config.host.build_root = Some(root.path().to_path_buf());
            config.host.cache_root_macos = match layout {
                "direct" => build.join("shared-cache"),
                "default-store-link" => root.path().join("cache"),
                "configured-store-link" => root.path().join("native-cache"),
                "case" => root.path().join("WORKSPACES").join(
                    build
                        .join("shared-cache")
                        .strip_prefix(root.path().join("workspaces"))
                        .unwrap(),
                ),
                _ => unreachable!(),
            };
            config.host.quota_bytes = u64::MAX;
            config.host.job_room_wait_seconds = 1;
            let store = if layout == "case" {
                config.host.cache_root_macos.join("verdict")
            } else {
                build.join("shared-cache/verdict")
            };
            fs::create_dir_all(&store).unwrap();
            if matches!(layout, "default-store-link" | "configured-store-link") {
                fs::create_dir_all(&config.host.cache_root_macos).unwrap();
                std::os::unix::fs::symlink(&store, config.host.cache_root_macos.join("verdict"))
                    .unwrap();
            }
            let same_workspace = layout != "case" || {
                let lower = fs::metadata(root.path().join("workspaces")).unwrap();
                let upper = fs::metadata(root.path().join("WORKSPACES")).unwrap();
                (lower.dev(), lower.ino()) == (upper.dev(), upper.ino())
            };
            let baseline = br#"{"runs":[{"sha":"main","tests":[],"jobs":[]}]}"#;
            fs::write(store.join("journal.json"), baseline).unwrap();
            assert_eq!(
                fs::read(config.host.cache_root_macos.join("verdict/journal.json")).unwrap(),
                baseline
            );

            let error = ensure_room_for_a_job(&config, root.path(), &config.host.verdict_stores())
                .unwrap_err();
            assert!(
                store.join("journal.json").is_file(),
                "{layout} (same identity={same_workspace}): job-side eviction removed the verdict journal"
            );
            assert_eq!(
                fs::read(store.join("journal.json")).unwrap(),
                baseline,
                "{layout}"
            );
            if !same_workspace {
                assert!(!build.join("debug/artifact").exists());
                continue;
            }
            assert!(build.join("debug/artifact").exists(), "{layout}");
            assert!(
                error
                    .to_string()
                    .contains(if matches!(layout, "direct" | "case") {
                        "must not overlap"
                    } else {
                        "traverses a symlink"
                    }),
                "{layout}: {error:#}"
            );
        }
    }

    /// A host under a build spends a whole cleanup pass's worth of space before
    /// the next pass fires, so a job arriving in that window used to be refused
    /// while evictable caches sat beside it. The gate reclaims them itself now;
    /// what it must not do is refuse first.
    #[cfg(unix)]
    #[test]
    fn the_gate_reclaims_caches_before_deciding_there_is_no_room() {
        let root = fixture_directory().unwrap();
        let build = build_beside_a_checkout(root.path(), "lint");

        reclaim(root.path());

        assert!(
            !build.join("debug/artifact").exists(),
            "an evictable build cache must be reclaimed, not left for the timer"
        );
    }

    /// A host deployed from a branch serves `production/main` until the branch
    /// merges, and main's lane slots under the cache root fill the volume the
    /// job asks room on. The gate reclaims them like the builds beside the
    /// checkouts.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_gate_reclaims_the_slots_the_previous_layout_keeps() {
        let root = fixture_directory().unwrap();
        let build = root
            .path()
            .join("cache")
            .join(consts::PREVIOUS_TARGET_SLOTS)
            .join("macos-aarch64-lane-lint-0/cargo/debug");
        fs::create_dir_all(&build).unwrap();
        fs::write(build.join("artifact"), [0_u8; 8]).unwrap();

        reclaim(root.path());

        assert!(!build.exists(), "an idle slot of main's must be reclaimed");
    }

    /// A sibling job holds the directory it builds into while its tests run.
    /// Cargo has long released `.cargo-lock` by then, so without the claim the
    /// reclaim reads the cache as abandoned and deletes the binaries the tests
    /// are still executing — 1869 of them failed to exec that way before this
    /// existed.
    #[cfg(unix)]
    #[test]
    fn a_leased_build_directory_is_left_alone_by_a_sibling_job() {
        let root = fixture_directory().unwrap();
        let build = build_beside_a_checkout(root.path(), "lint");

        let held = lease::hold(&build).expect("the running job claims its build");

        reclaim(root.path());

        assert!(
            build.join("debug/artifact").exists(),
            "a cache a job is building into must survive another job's reclaim"
        );
        drop(held);

        reclaim(root.path());

        assert!(
            !build.join("debug/artifact").exists(),
            "once the job is gone its cache is evictable again"
        );
    }

    /// A job asks for room on the volume it builds on, and a refusal names
    /// it: the cache share's free space says nothing about the disk the
    /// build fills.
    #[test]
    fn a_refused_job_names_the_volume_it_builds_on() {
        let volume = fixture_directory().unwrap();
        let builds = fixture_directory().unwrap();
        let mut config = super::super::config::fixture();
        config.host.quota_bytes = u64::MAX;
        config.host.reject_bytes = 0;
        config.host.job_room_wait_seconds = 0;
        config.host.build_root = Some(builds.path().to_path_buf());

        let error = ensure_room_for_a_job(&config, volume.path(), &config.host.verdict_stores())
            .expect_err("no volume has u64::MAX bytes free");

        assert!(
            error
                .to_string()
                .contains(&volume.path().display().to_string()),
            "the refusal must name the volume the job builds on: {error}"
        );
    }

    #[test]
    fn a_refusal_with_nothing_to_reclaim_does_not_claim_it_reclaimed() {
        let message = refusal(
            FsPath::new("/ci/builds/kithara.target/lint"),
            10,
            20,
            FsPath::new("/ci/workspaces/gitlab"),
            0,
        );

        assert!(
            !message.contains("after reclaiming"),
            "a reclaim that never had a candidate must not be reported as done: {message}"
        );
        assert!(
            message.contains("no reclaimable build cache sits under /ci/workspaces/gitlab"),
            "the refusal must name the root it found nothing under: {message}"
        );
    }

    #[test]
    fn a_refusal_that_reclaimed_says_how_much_it_had_to_work_with() {
        let message = refusal(
            FsPath::new("/ci/builds/kithara.target/lint"),
            10,
            20,
            FsPath::new("/ci/workspaces/gitlab"),
            3,
        );

        assert!(
            message.contains("after reclaiming from 3 build cache(s)"),
            "the refusal must say how many caches it emptied: {message}"
        );
    }

    #[test]
    fn a_workspace_root_that_does_not_exist_has_nothing_to_reclaim() {
        let root = fixture_directory().unwrap();

        let reclaimed_from = reclaim(root.path());

        assert_eq!(reclaimed_from, 0);
    }
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn mounted_cache_preparation_needs_no_parent_namespace() {
        let root = fixture_directory().unwrap();
        let tree = root.path().join("cache");
        let namespace = tree.join("review");
        fs::write(root.path().join(".kithara-job-leases"), b"outside mount").unwrap();
        assert!(claim_namespaces(&[(&tree, &namespace)]).unwrap().is_empty());
        assert!(!namespace.exists());
    }

    #[test]
    fn namespace_guards_protect_ancestors_before_creation() {
        let root = fixture_directory().unwrap();
        let tree = root.path().join("cache");
        let namespace = tree.join("review");
        let locks = namespace_lock_root(&tree).unwrap();
        let key = namespace_key(&tree, &namespace).unwrap();
        let guard = lease::claim(&locks, &key).unwrap();
        assert!(!tree.exists());
        assert!(lease::exclude(&locks, &key).unwrap().is_none());
        assert!(
            lease::exclude(
                &locks,
                &namespace_key(&tree, &tree.join("trusted")).unwrap()
            )
            .unwrap()
            .is_some()
        );
        assert!(namespace_key(&tree, &tree.join("../outside")).is_err());
        assert!(namespace_key(&tree, &root.path().join("outside")).is_err());
        drop(guard);
        assert!(lease::exclude(&locks, &key).unwrap().is_some());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unknown_historical_owners_survive_while_native_peers_reclaim() {
        let root = fixture_directory().unwrap();
        let native = build_beside_a_checkout(root.path(), "lint");
        let historical = root
            .path()
            .join("cache")
            .join(consts::PREVIOUS_TARGET_SLOTS)
            .join("unknown/cargo");
        let bootstrap = root
            .path()
            .join("cache")
            .join(consts::PREVIOUS_BOOTSTRAP)
            .join("review/linux-aarch64");
        for path in [&historical, &bootstrap] {
            fs::create_dir_all(path).unwrap();
            fs::write(path.join("sentinel"), b"uncertain").unwrap();
            fs::File::create(path.join(lease::HEARTBEAT))
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH)
                .unwrap();
        }
        reclaim(root.path());
        assert!(!native.exists());
        for path in [&historical, &bootstrap] {
            assert_eq!(fs::read(path.join("sentinel")).unwrap(), b"uncertain");
        }
    }
}
