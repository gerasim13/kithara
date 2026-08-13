use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use reqwest::Url;
use serde::{Deserialize, Serialize};

/// Directory every Unix executor reads the installed host profile from.
pub(crate) const LANE_CONFIG_DIR: &str = "/etc/kithara-ci";

/// Installed profile of the Mac mini and the guests it hosts, read through
/// `KITHARA_CI_HOST_CONFIG`. A Linux machine carries its own; see
/// [`crate::ci::linux`].
pub(crate) const MAC_CONFIG_PATH: &str = "/etc/kithara-ci/mac-host.toml";

/// Machine profile of one CI host: volumes, accounts, and installed roots.
/// It is provisioned per machine and never tracked in the repository; the
/// reviewed build contract lives in [`super::CiPins`].
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CiHost {
    pub(crate) admin_user: String,
    pub(crate) aggressive_cleanup_bytes: u64,
    pub(crate) android_home: PathBuf,
    pub(crate) brew_root: PathBuf,
    /// Positive decimal gigabytes, such as `25GB`. Defaulted: installed
    /// profiles predate it, and refusing to load would kill the cleanup.
    #[serde(default = "default_build_cache_size")]
    pub(crate) build_cache_size: String,
    pub(crate) cache_root_macos: PathBuf,
    pub(crate) cache_root_linux: PathBuf,
    pub(crate) cache_root_windows: PathBuf,
    pub(crate) ci_uid: u32,
    pub(crate) ci_user: String,
    pub(crate) macos_guest_shared_root: PathBuf,
    pub(crate) macos_guest_user: String,
    /// Locally built macOS VM bundle cloned for every job.
    pub(crate) macos_vm_bundle: PathBuf,
    pub(crate) macos_guest_xcode_developer_dir: PathBuf,
    pub(crate) gitlab_url: Url,
    pub(crate) host_root: PathBuf,
    /// Where runners check work out. Separate from `host_root` because Apple's
    /// packaging cannot run on a case-sensitive volume — `xcodebuild` writes
    /// `Headers` and `cargo swift package` then removes `headers` — while the
    /// rest of the host root is happy either way. Defaults to `host_root` for
    /// a machine whose volume already folds case.
    #[serde(default)]
    pub(crate) build_root: Option<PathBuf>,
    pub(crate) host_xcode_developer_dir: PathBuf,
    pub(crate) quota_bytes: u64,
    pub(crate) reject_bytes: u64,
    pub(crate) sccache_size: String,
    pub(crate) soft_cleanup_bytes: u64,
    pub(crate) sync_uid: u32,
    pub(crate) sync_user: String,
}

impl CiHost {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("reading CI host profile {}", path.display()))?;
        let host: Self = toml::from_str(&text)
            .with_context(|| format!("parsing CI host profile {}", path.display()))?;
        host.validate()?;
        Ok(host)
    }

    pub(crate) fn write(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self).context("serializing CI host profile")?;
        fs::write(path, text).with_context(|| format!("writing CI host profile {}", path.display()))
    }

    pub(crate) fn validate(&self) -> Result<()> {
        for (name, path) in [
            ("android_home", &self.android_home),
            ("brew_root", &self.brew_root),
            ("cache_root_macos", &self.cache_root_macos),
            ("cache_root_linux", &self.cache_root_linux),
            ("cache_root_windows", &self.cache_root_windows),
            ("macos_guest_shared_root", &self.macos_guest_shared_root),
            ("macos_vm_bundle", &self.macos_vm_bundle),
            (
                "macos_guest_xcode_developer_dir",
                &self.macos_guest_xcode_developer_dir,
            ),
            ("host_root", &self.host_root),
            ("host_xcode_developer_dir", &self.host_xcode_developer_dir),
        ] {
            if path.as_os_str().is_empty() {
                bail!("CI host profile {name} must not be empty");
            }
        }
        if self.sccache_size.trim().is_empty() {
            bail!("CI host profile sccache_size must not be empty");
        }
        if self.build_cache_size.trim().is_empty() {
            bail!("CI host profile build_cache_size must not be empty");
        }
        self.build_cache_budget_bytes()?;
        if self.soft_cleanup_bytes == 0
            || self.soft_cleanup_bytes >= self.aggressive_cleanup_bytes
            || self.aggressive_cleanup_bytes >= self.reject_bytes
            || self.reject_bytes >= self.quota_bytes
        {
            bail!("CI disk thresholds must satisfy 0 < soft < aggressive < reject < quota");
        }
        if self.ci_uid == 0 || self.sync_uid == 0 || self.ci_uid == self.sync_uid {
            bail!("CI and synchronization UIDs must be distinct non-root values");
        }
        if self.gitlab_url.scheme() != "https"
            || self.gitlab_url.host_str().is_none()
            || !self.gitlab_url.username().is_empty()
            || self.gitlab_url.password().is_some()
            || self.gitlab_url.query().is_some()
            || self.gitlab_url.fragment().is_some()
        {
            bail!("gitlab_url must be an HTTPS origin without credentials or query");
        }
        for (name, user) in [
            ("admin_user", self.admin_user.as_str()),
            ("ci_user", self.ci_user.as_str()),
            ("macos_guest_user", self.macos_guest_user.as_str()),
            ("sync_user", self.sync_user.as_str()),
        ] {
            if !safe_account(user) {
                bail!("CI host profile {name} contains unsupported characters");
            }
        }
        Ok(())
    }

    pub(crate) fn validate_macos_layout(&self) -> Result<()> {
        self.validate()?;
        for (name, path) in [
            ("android_home", &self.android_home),
            ("brew_root", &self.brew_root),
            ("cache_root_macos", &self.cache_root_macos),
            ("macos_guest_shared_root", &self.macos_guest_shared_root),
            ("macos_vm_bundle", &self.macos_vm_bundle),
            (
                "macos_guest_xcode_developer_dir",
                &self.macos_guest_xcode_developer_dir,
            ),
            ("host_root", &self.host_root),
            ("host_xcode_developer_dir", &self.host_xcode_developer_dir),
        ] {
            if !path.is_absolute() {
                bail!("CI host profile {name} must be an absolute macOS path");
            }
        }
        if self.host_root.parent() != Some(Path::new("/Volumes")) {
            bail!("host_root must name a dedicated volume directly below /Volumes");
        }
        if self
            .build_root
            .as_ref()
            .is_some_and(|build_root| !build_root.is_absolute())
        {
            bail!("CI host profile build_root must be an absolute macOS path");
        }
        Ok(())
    }

    pub(crate) fn gitlab_origin(&self) -> String {
        self.gitlab_url.as_str().trim_end_matches('/').to_string()
    }

    pub(crate) fn build_root(&self) -> &Path {
        self.build_root.as_deref().unwrap_or(&self.host_root)
    }

    pub(crate) fn build_cache_budget_bytes(&self) -> Result<u64> {
        parse_build_cache_size(&self.build_cache_size)
            .context("CI host profile build_cache_size is invalid")
    }

    /// The headroom a job insists on before it starts. The host stops handing
    /// out work once the CI volume passes `reject_bytes`, so the room left at
    /// that point is what the policy already considers too little to start on.
    pub(crate) const fn free_bytes_for_a_job(&self) -> u64 {
        self.quota_bytes.saturating_sub(self.reject_bytes)
    }

    /// `tart` resolves VM names under `TART_HOME`, and the configured bundle
    /// is `<TART_HOME>/vms/<name>`. A launch agent inherits none of the
    /// shell's environment, so without this it looks in `~/.tart` and cannot
    /// see the base image at all.
    pub(crate) fn tart_home(&self) -> Result<&Path> {
        self.macos_vm_bundle
            .parent()
            .and_then(Path::parent)
            .context("macos_vm_bundle must be <TART_HOME>/vms/<name>")
    }

    /// The guest mounts this bundle instead of carrying its own Xcode, so the
    /// pinned version is whatever the host has, and the image stays small.
    pub(crate) fn host_xcode_app(&self) -> Result<&Path> {
        self.host_xcode_developer_dir
            .parent()
            .and_then(Path::parent)
            .filter(|bundle| bundle.extension().is_some_and(|kind| kind == "app"))
            .context("host_xcode_developer_dir must be <Xcode>.app/Contents/Developer")
    }

    /// `launchd` starts agents with a minimal PATH, so any agent that shells
    /// out to a Homebrew or Cargo tool has to be told where they live.
    pub(crate) fn agent_path(&self, home: &Path) -> String {
        format!(
            "{}:{}:{}:/usr/bin:/bin:/usr/sbin:/sbin",
            self.brew_root.join("bin").display(),
            self.brew_root.join("sbin").display(),
            home.join(".cargo/bin").display()
        )
    }

    pub(crate) fn brew_tool(&self, name: &str) -> PathBuf {
        self.brew_root.join("bin").join(name)
    }

    pub(crate) fn java_home(&self) -> PathBuf {
        self.brew_root
            .join("opt/openjdk@17/libexec/openjdk.jdk/Contents/Home")
    }
}

#[derive(Debug)]
pub(crate) struct BuildCacheSizeError {
    value: String,
}

impl fmt::Display for BuildCacheSizeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "build_cache_size {:?} must be a positive whole number followed by GB and fit in u64 bytes",
            self.value
        )
    }
}

impl Error for BuildCacheSizeError {}

/// Budget a profile inherits when it predates the field. Sized so the Linux
/// host's two dozen runner caches fit its volume with room to spare, and so a
/// single macOS cache root stays well inside its own.
pub(crate) fn default_build_cache_size() -> String {
    "25GB".to_owned()
}

pub(crate) fn parse_build_cache_size(value: &str) -> Result<u64, BuildCacheSizeError> {
    const BYTES_PER_GIGABYTE: u64 = 1_000_000_000;

    let invalid = || BuildCacheSizeError {
        value: value.to_owned(),
    };
    let digits = value.strip_suffix("GB").ok_or_else(&invalid)?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    digits
        .parse::<u64>()
        .ok()
        .filter(|gigabytes| *gigabytes > 0)
        .and_then(|gigabytes| gigabytes.checked_mul(BYTES_PER_GIGABYTE))
        .ok_or_else(invalid)
}

fn safe_account(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_cache_size_converts_decimal_gigabytes_to_bytes() {
        assert_eq!(parse_build_cache_size("25GB").unwrap(), 25_000_000_000);
    }

    #[test]
    fn build_cache_size_rejects_garbage() {
        assert!(parse_build_cache_size("twenty-five").is_err());
    }

    #[test]
    fn ci_host_rejects_an_empty_build_cache_size() {
        let mut host = super::super::fixture().host;
        host.build_cache_size.clear();

        assert!(host.validate().is_err());
    }

    #[test]
    fn ci_host_load_accepts_a_profile_without_a_build_cache_size() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.toml");
        let host = super::super::fixture().host;
        host.write(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let without: String = text
            .lines()
            .filter(|line| !line.trim_start().starts_with("build_cache_size"))
            .map(|line| format!("{line}\n"))
            .collect();
        fs::write(&path, without).unwrap();

        assert_eq!(
            CiHost::load(&path).unwrap().build_cache_size,
            default_build_cache_size()
        );
    }

    #[test]
    fn ci_host_load_rejects_garbage_build_cache_size() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.toml");
        let mut host = super::super::fixture().host;
        host.build_cache_size = "25GiB".to_owned();
        host.write(&path).unwrap();

        let error = CiHost::load(&path).unwrap_err();

        assert!(format!("{error:#}").contains("positive whole number followed by GB"));
    }

    #[test]
    fn macos_build_root_must_be_absolute() {
        let mut host = super::super::fixture().host;
        host.build_root = Some(PathBuf::from("case-folding-build-root"));

        let error = host.validate_macos_layout().unwrap_err();

        assert!(format!("{error:#}").contains("build_root must be an absolute macOS path"));
    }

    #[test]
    fn account_names_are_bounded() {
        assert!(safe_account("kithara-ci"));
        assert!(!safe_account("../root"));
        assert!(!safe_account("ci user"));
    }

    /// A job cannot see the CI volume the way the host does, so it asks for
    /// room instead of occupancy. The room it asks for is the same policy read
    /// from the other end, and the ordering `validate` enforces keeps it above
    /// zero.
    #[test]
    fn a_job_asks_for_the_room_the_host_policy_reserves() {
        let mut host = super::super::fixture().host;
        host.soft_cleanup_bytes = 240;
        host.aggressive_cleanup_bytes = 270;
        host.reject_bytes = 285;
        host.quota_bytes = 300;
        assert_eq!(host.free_bytes_for_a_job(), 15);
        assert!(host.validate().is_ok());
        assert!(host.free_bytes_for_a_job() > 0);
    }
}
