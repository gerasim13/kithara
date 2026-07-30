use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use reqwest::Url;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CiConfig {
    pub(crate) cache_root_cilicon: PathBuf,
    pub(crate) cache_root_linux: PathBuf,
    pub(crate) cache_root_windows: PathBuf,
    pub(crate) host_root: PathBuf,
    pub(crate) quota_bytes: u64,
    pub(crate) reject_bytes: u64,
    pub(crate) soft_cleanup_bytes: u64,
    pub(crate) aggressive_cleanup_bytes: u64,
    pub(crate) stable_toolchain: String,
    pub(crate) msrv_toolchain: String,
    pub(crate) nightly_toolchain: String,
    pub(crate) expected_xcode_version: String,
    pub(crate) android_home: PathBuf,
    pub(crate) android_build_tools_version: String,
    pub(crate) android_commandline_tools_sha256: String,
    pub(crate) android_commandline_tools_version: String,
    pub(crate) android_ndk_version: String,
    pub(crate) android_platform_version: u32,
    pub(crate) android_avd: String,
    pub(crate) brew_root: PathBuf,
    pub(crate) brew_formulae: Vec<String>,
    pub(crate) brew_casks: Vec<String>,
    pub(crate) cargo_tools: BTreeMap<String, String>,
    pub(crate) cilicon_guest_shared_root: PathBuf,
    pub(crate) cilicon_version: String,
    pub(crate) cilicon_sha256: String,
    pub(crate) cilicon_ssh_user: String,
    pub(crate) sccache_size: String,
    pub(crate) ci_user: String,
    pub(crate) ci_uid: u32,
    pub(crate) sync_user: String,
    pub(crate) sync_uid: u32,
    pub(crate) admin_user: String,
    pub(crate) gitlab_url: Url,
    pub(crate) gitlab_runner_version: String,
    pub(crate) geckodriver_linux_arm64_sha256: String,
    pub(crate) geckodriver_version: String,
    pub(crate) gitleaks_linux_arm64_sha256: String,
    pub(crate) gitleaks_version: String,
    pub(crate) linux_base_image: String,
    pub(crate) linux_image: String,
    pub(crate) cilicon_image: String,
    pub(crate) cilicon_image_digest: String,
    pub(crate) cilicon_xcode_developer_dir: PathBuf,
    pub(crate) host_xcode_developer_dir: PathBuf,
}

impl CiConfig {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("reading CI host config {}", path.display()))?;
        let config: Self = serde_json::from_str(&text)
            .with_context(|| format!("parsing CI host config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub(crate) fn write(&self, path: &Path) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(self).context("serializing CI host config")?;
        fs::write(path, [bytes.as_slice(), b"\n"].concat())
            .with_context(|| format!("writing CI host config {}", path.display()))
    }

    pub(crate) fn validate(&self) -> Result<()> {
        for (name, path) in [
            ("cache_root_cilicon", &self.cache_root_cilicon),
            ("cache_root_linux", &self.cache_root_linux),
            ("cache_root_windows", &self.cache_root_windows),
        ] {
            if path.as_os_str().is_empty() {
                bail!("CI host config {name} must not be empty");
            }
        }
        for (name, path) in [
            ("host_root", &self.host_root),
            ("android_home", &self.android_home),
            ("brew_root", &self.brew_root),
            ("cilicon_guest_shared_root", &self.cilicon_guest_shared_root),
            (
                "cilicon_xcode_developer_dir",
                &self.cilicon_xcode_developer_dir,
            ),
            ("host_xcode_developer_dir", &self.host_xcode_developer_dir),
        ] {
            if path.as_os_str().is_empty() {
                bail!("CI host config {name} must not be empty");
            }
        }
        for (name, value) in [
            ("stable_toolchain", self.stable_toolchain.as_str()),
            ("msrv_toolchain", self.msrv_toolchain.as_str()),
            ("nightly_toolchain", self.nightly_toolchain.as_str()),
            (
                "expected_xcode_version",
                self.expected_xcode_version.as_str(),
            ),
            ("android_ndk_version", self.android_ndk_version.as_str()),
            (
                "android_build_tools_version",
                self.android_build_tools_version.as_str(),
            ),
            (
                "android_commandline_tools_sha256",
                self.android_commandline_tools_sha256.as_str(),
            ),
            (
                "android_commandline_tools_version",
                self.android_commandline_tools_version.as_str(),
            ),
            ("android_avd", self.android_avd.as_str()),
            ("cilicon_version", self.cilicon_version.as_str()),
            ("cilicon_sha256", self.cilicon_sha256.as_str()),
            ("cilicon_ssh_user", self.cilicon_ssh_user.as_str()),
            ("sccache_size", self.sccache_size.as_str()),
            ("ci_user", self.ci_user.as_str()),
            ("sync_user", self.sync_user.as_str()),
            ("admin_user", self.admin_user.as_str()),
            ("gitlab_runner_version", self.gitlab_runner_version.as_str()),
            ("geckodriver_version", self.geckodriver_version.as_str()),
            ("gitleaks_version", self.gitleaks_version.as_str()),
            ("linux_base_image", self.linux_base_image.as_str()),
            ("linux_image", self.linux_image.as_str()),
            ("cilicon_image", self.cilicon_image.as_str()),
            ("cilicon_image_digest", self.cilicon_image_digest.as_str()),
        ] {
            if value.trim().is_empty() {
                bail!("CI host config {name} must not be empty");
            }
        }
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
        if self.android_platform_version == 0 {
            bail!("android_platform_version must be non-zero");
        }
        if self.brew_formulae.is_empty()
            || self.cargo_tools.is_empty()
            || self
                .brew_formulae
                .iter()
                .chain(&self.brew_casks)
                .chain(self.cargo_tools.keys())
                .any(|value| value.trim().is_empty())
            || self
                .cargo_tools
                .values()
                .any(|value| value.trim().is_empty())
        {
            bail!("CI tool package names and versions must not be empty");
        }
        for (name, digest) in [
            (
                "android_commandline_tools_sha256",
                self.android_commandline_tools_sha256.as_str(),
            ),
            ("cilicon_sha256", self.cilicon_sha256.as_str()),
            (
                "geckodriver_linux_arm64_sha256",
                self.geckodriver_linux_arm64_sha256.as_str(),
            ),
            (
                "gitleaks_linux_arm64_sha256",
                self.gitleaks_linux_arm64_sha256.as_str(),
            ),
        ] {
            if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                bail!("CI host config {name} must be a SHA-256 digest");
            }
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
        let expected_image_prefix = format!("rust:{}-", self.stable_toolchain);
        let Some((base_image, image_digest)) = self.linux_base_image.split_once("@sha256:") else {
            bail!("linux_base_image must include an immutable SHA-256 digest");
        };
        if !base_image.starts_with(&expected_image_prefix)
            || image_digest.len() != 64
            || !image_digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            bail!("linux_base_image must pin the configured stable Rust toolchain by digest");
        }
        for (name, user) in [
            ("ci_user", self.ci_user.as_str()),
            ("sync_user", self.sync_user.as_str()),
            ("admin_user", self.admin_user.as_str()),
            ("cilicon_ssh_user", self.cilicon_ssh_user.as_str()),
        ] {
            if !safe_account(user) {
                bail!("CI host config {name} contains unsupported characters");
            }
        }
        Ok(())
    }

    pub(crate) fn validate_host(&self) -> Result<()> {
        self.validate()?;
        for (name, path) in [
            ("host_root", &self.host_root),
            ("android_home", &self.android_home),
            ("cache_root_cilicon", &self.cache_root_cilicon),
            ("brew_root", &self.brew_root),
            ("cilicon_guest_shared_root", &self.cilicon_guest_shared_root),
            (
                "cilicon_xcode_developer_dir",
                &self.cilicon_xcode_developer_dir,
            ),
            ("host_xcode_developer_dir", &self.host_xcode_developer_dir),
        ] {
            if !path.is_absolute() {
                bail!("CI host config {name} must be an absolute macOS path");
            }
        }
        if self.host_root.parent() != Some(Path::new("/Volumes")) {
            bail!("host_root must name a dedicated volume directly below /Volumes");
        }
        Ok(())
    }

    pub(crate) fn gitlab_origin(&self) -> String {
        self.gitlab_url.as_str().trim_end_matches('/').to_string()
    }

    pub(crate) fn brew_tool(&self, name: &str) -> PathBuf {
        self.brew_root.join("bin").join(name)
    }

    pub(crate) fn java_home(&self) -> PathBuf {
        self.brew_root
            .join("opt/openjdk@17/libexec/openjdk.jdk/Contents/Home")
    }

    pub(crate) fn gitlab_host(&self) -> Result<&str> {
        self.gitlab_url
            .host_str()
            .context("gitlab_url has no hostname")
    }

    pub(crate) fn cargo_tool_version(&self, name: &str) -> Result<&str> {
        self.cargo_tools
            .get(name)
            .map(String::as_str)
            .with_context(|| format!("CI host config has no cargo tool named {name}"))
    }
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
    fn tracked_host_config_is_valid_on_every_build_platform() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".config/ci-host.json");
        CiConfig::load(&path).unwrap();
    }

    #[test]
    fn account_names_are_bounded() {
        assert!(safe_account("kithara-ci"));
        assert!(!safe_account("../root"));
        assert!(!safe_account("ci user"));
    }
}
