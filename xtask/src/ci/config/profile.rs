use std::path::Path;

use anyhow::Result;

use super::{host::CiHost, pins::CiPins};

/// One CI machine described by its own profile plus the reviewed build pins.
#[derive(Clone, Debug)]
pub(crate) struct CiConfig {
    pub(crate) host: CiHost,
    pub(crate) pins: CiPins,
}

impl CiConfig {
    pub(crate) fn load(host: &Path, pins: &Path) -> Result<Self> {
        Ok(Self {
            host: CiHost::load(host)?,
            pins: CiPins::load(pins)?,
        })
    }

    pub(crate) fn validate(&self) -> Result<()> {
        self.host.validate()?;
        self.pins.validate()
    }

    pub(crate) fn validate_macos_layout(&self) -> Result<()> {
        self.host.validate_macos_layout()?;
        self.pins.validate()
    }
}

#[cfg(test)]
pub(crate) fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace")
}

#[cfg(test)]
pub(crate) fn fixture() -> CiConfig {
    CiConfig::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ci-host.json"),
        &workspace_root().join(".config/ci-pins.json"),
    )
    .expect("fixture host profile and tracked pins must load")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracked_pins_are_valid_on_every_build_platform() {
        CiPins::load(&workspace_root().join(".config/ci-pins.json")).unwrap();
    }

    #[test]
    fn fixture_profile_matches_the_host_contract() {
        fixture().validate_macos_layout().unwrap();
    }
}
