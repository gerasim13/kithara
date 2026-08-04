use std::{
    collections::BTreeMap,
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use kithara_devtools::Ctx;
use nix::sys::resource::{Resource, getrlimit, setrlimit};

use super::{config::CiConfig, run::Lane};

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

        let used_bytes = used_bytes(&shared_root)?;
        if is_ci() && used_bytes >= config.host.reject_bytes {
            bail!(
                "CI volume uses {used_bytes} bytes; new jobs are blocked at {} bytes",
                config.host.reject_bytes
            );
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

        for directory in [
            &cache_root,
            &cargo_home,
            &gradle_home,
            &fixture_cache,
            &npm_cache,
            &sccache_dir,
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
        set_path(&mut vars, &home, config)?;
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
        insert(&mut vars, "RUSTC_WRAPPER", "sccache");
        insert(
            &mut vars,
            "RUSTUP_HOME",
            env::var_os("RUSTUP_HOME").unwrap_or_else(|| home.join(".rustup").into_os_string()),
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

fn raise_open_file_limit() -> Result<()> {
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

fn shared_root(config: &CiConfig, lane: Lane) -> PathBuf {
    if let Some(root) = env::var_os("KITHARA_CI_CACHE_ROOT") {
        return PathBuf::from(root);
    }
    match lane {
        Lane::Apple | Lane::Deep | Lane::ReleaseApple => config.host.cache_root_macos.clone(),
        Lane::Linux | Lane::Web | Lane::Weekly => config.host.cache_root_linux.clone(),
        Lane::Windows => config.host.cache_root_windows.clone(),
        Lane::Android | Lane::ReleaseAndroid | Lane::ReleasePublish => {
            config.host.host_root.join("cache")
        }
    }
}

fn is_ci() -> bool {
    env::var_os("CI").is_some_and(|value| !value.is_empty())
}

fn used_bytes(path: &Path) -> Result<u64> {
    let total = fs4::total_space(path)
        .with_context(|| format!("reading total space for {}", path.display()))?;
    let available = fs4::available_space(path)
        .with_context(|| format!("reading available space for {}", path.display()))?;
    Ok(total.saturating_sub(available))
}

fn set_path(vars: &mut BTreeMap<OsString, OsString>, home: &Path, config: &CiConfig) -> Result<()> {
    let mut paths = vec![home.join(".cargo/bin")];
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
