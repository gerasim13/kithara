use std::{
    collections::BTreeSet,
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Installed profile every Linux CI machine reads through
/// `KITHARA_CI_LINUX_CONFIG`.
pub(crate) const LINUX_CONFIG_PATH: &str = "/etc/kithara-ci/linux-host.toml";

/// Machine profile of one Linux CI host: the runners it serves and what each of
/// them may consume. It is provisioned per machine and never tracked in the
/// repository; the reviewed build contract lives in
/// [`crate::ci::config::CiPins`].
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LinuxHost {
    /// Directory holding the caches jobs keep between runs.
    pub(crate) cache_root: PathBuf,
    /// Docker network the runners are confined to.
    pub(crate) network: String,
    /// `owner/name` of the repository the runners register with.
    pub(crate) repository: String,
    /// Address block of that network, fenced off from the rest of the machine.
    pub(crate) subnet: String,
    /// File holding the token that mints runner registrations.
    pub(crate) token_file: PathBuf,
    /// Serialised last: TOML requires tables after plain values.
    pub(crate) runners: Vec<LinuxRunner>,
    /// The Windows guest this machine hosts, if it hosts one.
    #[serde(default)]
    pub(crate) windows: Option<WindowsGuest>,
}

/// A Windows virtual machine serving the lane that needs a real Windows.
///
/// Cross-compiling to MSVC answers whether the code builds; only a guest
/// answers whether it runs.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WindowsGuest {
    pub(crate) name: String,
    pub(crate) vcpus: u32,
    pub(crate) memory_mib: u32,
    pub(crate) disk_gib: u32,
    /// libvirt network the guest is attached to. This is not the Docker
    /// network the containers use: the two live in different worlds and a
    /// name that exists in one means nothing in the other.
    #[serde(default = "default_libvirt_network")]
    pub(crate) network: String,
}

fn default_libvirt_network() -> String {
    "default".to_owned()
}

/// One runner served by this machine.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LinuxRunner {
    /// Identifies the runner's service, registration, and container.
    pub(crate) name: String,
    /// How much of the machine one job may take.
    pub(crate) cpus: String,
    pub(crate) memory: String,
    /// Labels a workflow selects this runner by.
    pub(crate) labels: Vec<String>,
    /// Host devices the job needs, such as `/dev/kvm` for an emulator.
    #[serde(default)]
    pub(crate) devices: Vec<PathBuf>,
    /// Groups the job joins on top of its own. A device node is typically
    /// owned by one, and its number is a property of the machine rather than
    /// of the image, which is why it is named here.
    #[serde(default)]
    pub(crate) groups: Vec<u32>,
    /// Which image this runner starts from.
    #[serde(default)]
    pub(crate) flavor: RunnerFlavor,
}

/// The images a runner can be built on. A plain runner carries the workspace
/// toolchain; an Android one carries an emulator on top of it.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) enum RunnerFlavor {
    #[default]
    Plain,
    Android,
}

impl LinuxHost {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("reading Linux CI profile {}", path.display()))?;
        let host: Self = toml::from_str(&text)
            .with_context(|| format!("parsing Linux CI profile {}", path.display()))?;
        host.validate()?;
        Ok(host)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if !self.cache_root.is_absolute() || !self.token_file.is_absolute() {
            bail!("Linux CI profile cache_root and token_file must be absolute paths");
        }
        if !safe_name(&self.network) {
            bail!("Linux CI profile network contains unsupported characters");
        }
        if !valid_repository(&self.repository) {
            bail!("Linux CI profile repository must read owner/name");
        }
        parse_subnet(&self.subnet)?;
        if self.runners.is_empty() {
            bail!("Linux CI profile must define at least one runner");
        }
        if let Some(windows) = &self.windows {
            windows.validate()?;
        }
        let mut seen = BTreeSet::new();
        for runner in &self.runners {
            runner.validate()?;
            if !seen.insert(runner.name.as_str()) {
                bail!("Linux CI profile defines runner {} twice", runner.name);
            }
        }
        Ok(())
    }

    pub(crate) fn runner(&self, name: &str) -> Result<&LinuxRunner> {
        self.runners
            .iter()
            .find(|runner| runner.name == name)
            .with_context(|| format!("Linux CI profile has no runner named {name}"))
    }
}

impl WindowsGuest {
    fn validate(&self) -> Result<()> {
        if !safe_name(&self.name) {
            bail!("Linux CI profile's Windows guest name is unusable");
        }
        // Windows 11 refuses to install below these, and a guest that installs
        // but cannot compile is worse than one that never started.
        if self.vcpus < 2 || self.memory_mib < 4096 || self.disk_gib < 64 {
            bail!("the Windows guest needs at least 2 vCPUs, 4 GiB, and 64 GiB");
        }
        if !safe_name(&self.network) {
            bail!("the Windows guest's network name is unusable");
        }
        Ok(())
    }
}

impl LinuxRunner {
    fn validate(&self) -> Result<()> {
        if !safe_name(&self.name) {
            bail!("Linux CI runner name contains unsupported characters");
        }
        if self.cpus.trim().is_empty() || self.memory.trim().is_empty() {
            bail!(
                "Linux CI runner {} must bound its CPU and memory",
                self.name
            );
        }
        if self.labels.is_empty() || self.labels.iter().any(|label| !safe_label(label)) {
            bail!("Linux CI runner {} has an unusable label", self.name);
        }
        if self.devices.iter().any(|device| !device.is_absolute()) {
            bail!("Linux CI runner {} names a relative device", self.name);
        }
        Ok(())
    }

    /// The systemd unit and container both carry the runner's own name.
    pub(crate) fn service(&self) -> String {
        format!("kithara-ci-{}.service", self.name)
    }

    pub(crate) fn labels(&self) -> String {
        self.labels.join(",")
    }
}

/// Docker accepts any CIDR here, so the parse doubles as the check that the
/// firewall rules built from it cannot be widened by a malformed profile.
fn parse_subnet(value: &str) -> Result<(Ipv4Addr, u32)> {
    let (address, prefix) = value
        .split_once('/')
        .with_context(|| format!("Linux CI profile subnet {value} is not a CIDR block"))?;
    let address: Ipv4Addr = address
        .parse()
        .with_context(|| format!("Linux CI profile subnet {value} has no IPv4 address"))?;
    let prefix: u32 = prefix
        .parse()
        .with_context(|| format!("Linux CI profile subnet {value} has no prefix length"))?;
    if !(8..=30).contains(&prefix) {
        bail!("Linux CI profile subnet {value} must have a prefix between 8 and 30");
    }
    Ok((address, prefix))
}

fn valid_repository(value: &str) -> bool {
    value
        .split_once('/')
        .is_some_and(|(owner, name)| safe_name(owner) && safe_name(name))
}

fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn safe_label(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A profile shaped like the machines this command provisions: one plain
    /// runner and one that reaches the hardware a plain one must not.
    pub(crate) fn host_fixture() -> LinuxHost {
        LinuxHost::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ci-linux-host.toml"),
        )
        .expect("the fixture profile must load")
    }

    #[test]
    fn the_fixture_profile_matches_the_machine_contract() {
        let host = host_fixture();
        assert!(host.runner("kithara-ci").is_ok());
        assert!(host.runner("absent").is_err());
    }

    #[test]
    fn subnets_are_bounded() {
        assert!(parse_subnet("172.16.240.0/24").is_ok());
        assert!(parse_subnet("172.16.240.0").is_err());
        assert!(parse_subnet("172.16.240.0/4").is_err());
        assert!(parse_subnet("not-an-address/24").is_err());
    }

    #[test]
    fn repositories_name_an_owner() {
        assert!(valid_repository("owner/kithara"));
        assert!(!valid_repository("kithara"));
        assert!(!valid_repository("owner/kithara/extra"));
        assert!(!valid_repository("owner/../etc"));
    }
}
