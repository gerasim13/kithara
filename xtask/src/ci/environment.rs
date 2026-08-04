use std::{
    collections::BTreeMap,
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use kithara_devtools::Ctx;

use super::{
    config::CiConfig,
    run::{CacheGroup, Lane},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CacheTrust {
    Quarantine,
    Review,
    Trusted,
}

impl CacheTrust {
    fn from_environment() -> Result<Self> {
        match env::var("KITHARA_CACHE_TRUST")
            .unwrap_or_else(|_| "review".into())
            .as_str()
        {
            "quarantine" => Ok(Self::Quarantine),
            "review" => Ok(Self::Review),
            "trusted" => Ok(Self::Trusted),
            value => bail!("unsupported KITHARA_CACHE_TRUST value: {value}"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Quarantine => "quarantine",
            Self::Review => "review",
            Self::Trusted => "trusted",
        }
    }
}

pub(crate) struct CiEnvironment {
    pub(crate) cache_root: PathBuf,
    pub(crate) swiftpm_cache: PathBuf,
    pub(crate) temp: PathBuf,
    active_marker: PathBuf,
    vars: BTreeMap<OsString, OsString>,
}

impl CiEnvironment {
    pub(crate) fn prepare(ctx: &Ctx, config: &CiConfig, lane: Lane) -> Result<Self> {
        config.validate()?;
        raise_open_file_limit()?;
        let home = env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .context("HOME or USERPROFILE must be set")?;
        let shared_root = shared_root(config, lane);
        let shared_root = if shared_root.is_dir() {
            shared_root
        } else if is_ci() {
            bail!(
                "shared CI cache is not mounted at {}",
                shared_root.display()
            );
        } else {
            home.join(".cache/kithara-ci")
        };
        fs::create_dir_all(&shared_root)
            .with_context(|| format!("creating CI cache root {}", shared_root.display()))?;

        let free_bytes = free_bytes(&shared_root)?;
        let required_free = config.host.free_bytes_for_a_job();
        if is_ci() && free_bytes < required_free {
            bail!("the CI cache has {free_bytes} bytes free; a job needs {required_free} bytes");
        }

        let trust = CacheTrust::from_environment()?;
        let platform = format!("{}-{}", env::consts::OS, env::consts::ARCH);
        let cache_root = shared_root.join(trust.as_str()).join(platform);
        let active_marker = cache_root.join(".kithara-ci-active");
        let cargo_home = cache_root.join("cargo");
        let gradle_home = cache_root.join("gradle");
        let fixture_cache = cache_root.join("fixtures");
        let npm_cache = cache_root.join("npm");
        let sccache_dir = cache_root.join("sccache");
        let swiftpm_cache = cache_root.join("swiftpm");
        let project_root =
            env::var_os("CI_PROJECT_DIR").map_or_else(|| ctx.root.clone(), PathBuf::from);
        let target = project_root.join("target");
        let temp = scratch_root().join(trust.as_str());
        // Where reuse accumulates. Executors that own their storage keep it in
        // the shared cache, so it spans jobs and sits on the CI volume rather
        // than the system disk; the throwaway guest keeps it locally because
        // its share cannot commit a blob.
        let reapi_cache = if lane.cache_group().keeps_reuse_store_in_shared_cache() {
            cache_root.join("reapi")
        } else {
            temp.join("reapi")
        };

        for directory in [
            &cache_root,
            &cargo_home,
            &gradle_home,
            &fixture_cache,
            &npm_cache,
            &sccache_dir,
            &reapi_cache,
            &swiftpm_cache,
            &temp,
        ] {
            fs::create_dir_all(directory)
                .with_context(|| format!("creating CI directory {}", directory.display()))?;
        }
        fs::write(
            &active_marker,
            format!(
                "pid={}\njob={}\n",
                std::process::id(),
                env::var("CI_JOB_ID").unwrap_or_else(|_| "local".into())
            ),
        )
        .with_context(|| format!("creating CI cache lease {}", active_marker.display()))?;

        let mut vars = BTreeMap::new();
        // Beside the reuse cache on the executor's own disk, for the same
        // reason and one more: the shim names the Cargo of the machine that
        // wrote it, and the host runner and the throwaway guest share one
        // `trusted/macos-aarch64` cache. Whichever wrote last decided for both,
        // so a guest job would exec a path that only exists on the host.
        let shim_directory = temp.join("shim");
        let cargo_shim =
            install_cargo_shim(&shim_directory, &resolve_cargo(&home, &shim_directory)?)?;
        set_path(&mut vars, &home, config, &cargo_shim)?;
        insert(&mut vars, "CARGO_HOME", cargo_home);
        insert(&mut vars, "CARGO_INCREMENTAL", "0");
        insert(&mut vars, "CARGO_TARGET_DIR", target);
        insert(&mut vars, "GRADLE_USER_HOME", gradle_home);
        insert(&mut vars, "KITHARA_FIXTURE_CACHE", fixture_cache);
        insert(
            &mut vars,
            "KITHARA_NIGHTLY_TOOLCHAIN",
            &config.pins.nightly_toolchain,
        );
        insert(&mut vars, "npm_config_cache", npm_cache);
        set_reuse_vars(&mut vars, &reapi_cache);
        insert(&mut vars, "RUSTC_WRAPPER", "sccache");
        insert(
            &mut vars,
            "RUSTUP_HOME",
            env::var_os("RUSTUP_HOME").unwrap_or_else(|| home.join(".rustup").into_os_string()),
        );
        insert(
            &mut vars,
            "KITHARA_REAPI_DECLARED_INPUTS",
            declared_inputs(config, &home),
        );
        insert(&mut vars, "SCCACHE_BASEDIRS", &project_root);
        insert(&mut vars, "SCCACHE_CACHE_SIZE", &config.host.sccache_size);
        insert(&mut vars, "SCCACHE_DIR", sccache_dir);
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
            let android_user_home = config.host.host_root.join("toolchains/android-user");
            insert(&mut vars, "ANDROID_HOME", &config.host.android_home);
            insert(
                &mut vars,
                "ANDROID_NDK_HOME",
                config
                    .host
                    .android_home
                    .join("ndk")
                    .join(&config.pins.android_ndk_version),
            );
            insert(&mut vars, "ANDROID_USER_HOME", &android_user_home);
            insert(&mut vars, "ANDROID_AVD_HOME", android_user_home.join("avd"));
            let java_home = config.host.java_home();
            if java_home.is_dir() {
                insert(&mut vars, "JAVA_HOME", &java_home);
            }
        }

        Ok(Self {
            cache_root,
            swiftpm_cache,
            temp,
            active_marker,
            vars,
        })
    }

    pub(crate) fn vars(&self) -> BTreeMap<OsString, OsString> {
        self.vars.clone()
    }
}

impl Drop for CiEnvironment {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.active_marker);
    }
}

/// Scratch space answers to three constraints at once. It sits outside the
/// checkout, or tools that walk the working tree — the architecture reporter,
/// for one — trip over the temporary copies they just created. It stays short,
/// because macOS caps Unix socket paths at `SUN_LEN` and the suite binds
/// sockets here. And it lives on local storage: the macOS guest reaches the
/// shared cache over virtiofs, which cannot bind a socket at all.
fn scratch_root() -> PathBuf {
    PathBuf::from("/tmp/kithara-ci")
}

/// The integration suite opens far more files across its cache and segment
/// fixtures than the 256 descriptor soft limit a macOS session starts with.
/// The lane raises its own ceiling so every executor gets the same budget.
const OPEN_FILES: u64 = 65536;

#[cfg(unix)]
fn raise_open_file_limit() -> Result<()> {
    use nix::sys::resource::{Resource, getrlimit, setrlimit};

    let (soft, hard) =
        getrlimit(Resource::RLIMIT_NOFILE).context("reading the file descriptor limit")?;
    let target = hard.min(OPEN_FILES);
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

fn shared_root(config: &CiConfig, lane: Lane) -> PathBuf {
    if let Some(root) = env::var_os("KITHARA_CI_CACHE_ROOT") {
        return PathBuf::from(root);
    }
    match lane.cache_group() {
        CacheGroup::Macos => config.host.cache_root_macos.clone(),
        CacheGroup::Linux => config.host.cache_root_linux.clone(),
        CacheGroup::Windows => config.host.cache_root_windows.clone(),
        CacheGroup::Host => config.host.host_root.join("cache"),
    }
}

fn is_ci() -> bool {
    env::var_os("CI").is_some_and(|value| !value.is_empty())
}

/// How much room the cache still has. A job reads this through whatever the
/// executor mounted the cache with — a virtiofs share into an ephemeral macOS
/// guest, a bind mount into a container — and those report the filesystem
/// backing the share, which is the host's whole disk rather than the CI volume.
/// Free space survives that translation and still answers the question a job
/// asks; occupancy does not, and comparing the host's disk against a threshold
/// sized for the CI volume rejected every macOS job while the volume was barely
/// half full.
fn free_bytes(path: &Path) -> Result<u64> {
    fs4::available_space(path)
        .with_context(|| format!("reading available space for {}", path.display()))
}

/// Route every Cargo invocation through `cargo reapi` by putting a shim ahead
/// of the real one. The tool wraps a Cargo command rather than installing a
/// compiler wrapper, and the lanes drive `just` recipes that each call Cargo
/// several times; naming the wrapper at every call site would mean editing
/// every recipe and would miss the next one written. The shim re-enters itself
/// when `cargo reapi` runs Cargo, so it hands over to the real binary whenever
/// the marker is already set.
///
/// Whole-gate snapshots stay off. They are the layer worth having — a hit
/// restores the build directory and skips Cargo entirely — but the strict
/// sandbox refuses to start Cargo when the cache backend drives it here, while
/// the same sandbox under the capture backend builds the workspace without
/// complaint. Until that is settled the per-action layer is what runs, and it
/// caches linked outputs, which the compiler cache underneath it cannot.
///
/// That compiler cache is dropped for the wrapped call: two caches claiming
/// `RUSTC_WRAPPER` cannot both have it.
/// Where the real Cargo lives on this executor. The macOS guest keeps it under
/// the CI account's home; the Linux image installs it system-wide with
/// `CARGO_HOME=/usr/local/cargo`, and a shim pointed at a home that has no
/// `.cargo` fails every job with `exec: cargo: not found`.
fn resolve_cargo(home: &Path, shim: &Path) -> Result<PathBuf> {
    let in_home = home.join(".cargo/bin/cargo");
    if in_home.is_file() {
        return Ok(in_home);
    }
    env::var_os("PATH")
        .map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        // A job that already ran here left the shim on PATH; picking it would
        // make the shim call itself.
        .filter(|directory| directory != shim)
        .map(|directory| directory.join("cargo"))
        .find(|candidate| candidate.is_file())
        .context("no cargo on PATH and none under the CI account's home")
}

/// This machine's memory, in whole gigabytes. Every executor reports it
/// somewhere different, and a machine that reports nowhere simply goes
/// unanswered — the caller then leaves the tool to its own default.
fn physical_memory_gib() -> Option<u64> {
    const GIB: u64 = 1024 * 1024 * 1024;
    if cfg!(target_os = "linux") {
        let text = fs::read_to_string("/proc/meminfo").ok()?;
        let kilobytes: u64 = text
            .lines()
            .find_map(|line| line.strip_prefix("MemTotal:"))?
            .split_whitespace()
            .next()?
            .parse()
            .ok()?;
        return Some(kilobytes * 1024 / GIB);
    }
    if cfg!(target_os = "macos") {
        let output = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        let bytes: u64 = String::from_utf8(output.stdout).ok()?.trim().parse().ok()?;
        return Some(bytes / GIB);
    }
    None
}

/// How the build-reuse tool is pointed at this executor: which store it
/// writes to, where its own log goes, and the ledger it admits work against.
fn set_reuse_vars(vars: &mut BTreeMap<OsString, OsString>, reapi_cache: &Path) {
    // Build reuse across the jobs an executor serves.
    insert(vars, "CARGO_REAPI_BACKEND", "cache");
    insert(vars, "CARGO_REAPI_CACHE_DIR", reapi_cache);
    // The action log defaults into `target/`, which is inside the tree the
    // tool's own sandbox governs: writing it back failed with "Operation
    // not permitted" for every standard-library build script, and on the
    // guest with a plain I/O error. It belongs beside the cache.
    insert(
        vars,
        "CARGO_REAPI_ACTION_LOG",
        reapi_cache.join("actions.jsonl"),
    );
    // The ledger the tool admits work against. It refuses to start when
    // told to plan for more memory than the machine has, and the executors
    // differ — twelve gigabytes in the macOS guest, eight in the container,
    // twenty-four on the host — so each states its own. It is an admission
    // cap rather than a reservation, and holding some back only narrowed
    // the window: a link the tool sizes at seven gigabytes then had nothing
    // to overlap with, and the heaviest lane stalled waiting for a lease.
    if let Some(gib) = physical_memory_gib() {
        insert(
            vars,
            "CARGO_REAPI_RESOURCE_MEMORY_GIB_CAPACITY",
            gib.to_string(),
        );
    }
    if let Ok(cores) = std::thread::available_parallelism() {
        insert(
            vars,
            "CARGO_REAPI_RESOURCE_CPU_CAPACITY",
            cores.get().to_string(),
        );
    }
}

/// Roots outside the checkout that a build legitimately reads: the platform
/// SDKs, the toolchains, and the package manager whose headers the vendored C
/// dependencies include. The reuse tool hides everything it was not told about.
fn declared_inputs(config: &CiConfig, home: &Path) -> OsString {
    let mut roots: Vec<PathBuf> = vec![
        home.join(".cargo"),
        home.join(".rustup"),
        config.host.host_root.join("toolchains"),
    ];
    if cfg!(target_os = "macos") {
        roots.extend([
            config.host.android_home.clone(),
            config.host.brew_root.clone(),
            config.host.macos_guest_xcode_developer_dir.clone(),
            config.host.host_xcode_developer_dir.clone(),
            PathBuf::from("/Library/Developer/CommandLineTools"),
        ]);
    }
    roots.retain(|root| root.is_dir());
    roots.dedup();
    let joined: Vec<String> = roots
        .iter()
        .map(|root| root.display().to_string())
        .collect();
    OsString::from(joined.join(":"))
}

fn install_cargo_shim(bin: &Path, cargo: &Path) -> Result<PathBuf> {
    fs::create_dir_all(bin)
        .with_context(|| format!("creating the Cargo shim directory {}", bin.display()))?;
    // The declared inputs come through the environment as a colon-separated
    // list: build scripts read headers and SDKs that live outside the
    // workspace — the Android NDK first among them — and the sandbox hides
    // anything it was not told about. `zstd-sys` reported it as a missing
    // `string.h`, which says nothing about a sandbox at all.
    let script = format!(
        "#!/bin/sh\n\
         if [ -n \"${{KITHARA_REAPI_ACTIVE:-}}\" ]; then exec {cargo} \"$@\"; fi\n\
         KITHARA_REAPI_ACTIVE=1 export KITHARA_REAPI_ACTIVE\n\
         unset RUSTC_WRAPPER\n\
         declared=\"\"\n\
         IFS=:\n\
         for root in ${{KITHARA_REAPI_DECLARED_INPUTS:-}}; do\n\
         \x20 [ -n \"$root\" ] && declared=\"$declared --declared-input $root\"\n\
         done\n\
         unset IFS\n\
         exec {cargo} reapi --snapshot-policy off $declared \"$@\"\n",
        cargo = cargo.display()
    );
    let shim = bin.join("cargo");
    fs::write(&shim, script)
        .with_context(|| format!("writing the Cargo shim {}", shim.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&shim, PermissionsExt::from_mode(0o755))
            .with_context(|| format!("making the Cargo shim executable {}", shim.display()))?;
    }
    Ok(bin.to_path_buf())
}

fn set_path(
    vars: &mut BTreeMap<OsString, OsString>,
    home: &Path,
    config: &CiConfig,
    shim: &Path,
) -> Result<()> {
    let mut paths = vec![shim.to_path_buf(), home.join(".cargo/bin")];
    if cfg!(target_os = "macos") {
        paths.extend([
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_trust_is_strict() {
        assert_eq!(CacheTrust::Review.as_str(), "review");
        assert_eq!(CacheTrust::Quarantine.as_str(), "quarantine");
        assert_eq!(CacheTrust::Trusted.as_str(), "trusted");
    }
}
