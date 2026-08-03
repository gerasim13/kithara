use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Repository-relative location of the reviewed build pins.
pub(crate) const PINS_PATH: &str = ".config/ci-pins.toml";

/// Reviewed build contract: everything a CI job installs, pulls, or pins.
/// Machine-specific paths and accounts live in [`super::CiHost`].
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CiPins {
    pub(crate) android_avd: String,
    pub(crate) android_build_tools_version: String,
    pub(crate) android_commandline_tools_sha256: String,
    pub(crate) android_commandline_tools_version: String,
    pub(crate) android_ndk_version: String,
    pub(crate) android_platform_version: u32,
    pub(crate) brew_casks: Vec<String>,
    pub(crate) brew_formulae: Vec<String>,
    pub(crate) cilicon_sha256: String,
    pub(crate) cilicon_version: String,
    pub(crate) expected_xcode_version: String,
    pub(crate) geckodriver_linux_arm64_sha256: String,
    pub(crate) geckodriver_version: String,
    pub(crate) gitlab_runner_version: String,
    pub(crate) gitleaks_linux_arm64_sha256: String,
    pub(crate) gitleaks_version: String,
    pub(crate) linux_base_digest: String,
    pub(crate) linux_image: String,
    /// Build the guest macOS must report. The CI VM is built locally from the
    /// matching Apple restore image instead of pulled from a registry.
    pub(crate) macos_guest_build: String,
    pub(crate) msrv_toolchain: String,
    pub(crate) nightly_toolchain: String,
    pub(crate) stable_toolchain: String,
    /// Serialised last: TOML requires tables after plain values.
    pub(crate) cargo_tools: BTreeMap<String, String>,
}

impl CiPins {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("reading CI pins {}", path.display()))?;
        let pins: Self =
            toml::from_str(&text).with_context(|| format!("parsing CI pins {}", path.display()))?;
        pins.validate()?;
        Ok(pins)
    }

    pub(crate) fn write(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self).context("serializing CI pins")?;
        fs::write(path, text).with_context(|| format!("writing CI pins {}", path.display()))
    }

    pub(crate) fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("android_avd", self.android_avd.as_str()),
            (
                "android_build_tools_version",
                self.android_build_tools_version.as_str(),
            ),
            (
                "android_commandline_tools_version",
                self.android_commandline_tools_version.as_str(),
            ),
            ("android_ndk_version", self.android_ndk_version.as_str()),
            ("cilicon_version", self.cilicon_version.as_str()),
            (
                "expected_xcode_version",
                self.expected_xcode_version.as_str(),
            ),
            ("geckodriver_version", self.geckodriver_version.as_str()),
            ("gitlab_runner_version", self.gitlab_runner_version.as_str()),
            ("gitleaks_version", self.gitleaks_version.as_str()),
            ("linux_image", self.linux_image.as_str()),
            ("macos_guest_build", self.macos_guest_build.as_str()),
            ("msrv_toolchain", self.msrv_toolchain.as_str()),
            ("nightly_toolchain", self.nightly_toolchain.as_str()),
            ("stable_toolchain", self.stable_toolchain.as_str()),
        ] {
            if value.trim().is_empty() {
                bail!("CI pin {name} must not be empty");
            }
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
            ("linux_base_digest", self.linux_base_digest.as_str()),
        ] {
            if !is_sha256(digest) {
                bail!("CI pin {name} must be a SHA-256 digest");
            }
        }
        Ok(())
    }

    pub(crate) fn cargo_tool_version(&self, name: &str) -> Result<&str> {
        self.cargo_tools
            .get(name)
            .map(String::as_str)
            .with_context(|| format!("CI pins have no cargo tool named {name}"))
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_are_bounded() {
        assert!(is_sha256(&"a".repeat(64)));
        assert!(!is_sha256(&"a".repeat(63)));
        assert!(!is_sha256(&"z".repeat(64)));
    }
}
