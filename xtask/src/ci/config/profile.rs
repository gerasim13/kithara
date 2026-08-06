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
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ci-mac-host.toml"),
        &workspace_root().join(".config/ci-pins.toml"),
    )
    .expect("fixture host profile and tracked pins must load")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracked_pins_are_valid_on_every_build_platform() {
        CiPins::load(&workspace_root().join(".config/ci-pins.toml")).unwrap();
    }

    #[test]
    fn fixture_profile_matches_the_host_contract() {
        fixture().validate_macos_layout().unwrap();
    }

    #[test]
    fn installed_copies_round_trip_through_toml() {
        let config = fixture();
        let directory = tempfile::tempdir().unwrap();
        let host = directory.path().join("mac-host.toml");
        let pins = directory.path().join("pins.toml");
        config.host.write(&host).unwrap();
        config.pins.write(&pins).unwrap();

        let installed = CiConfig::load(&host, &pins).unwrap();
        assert_eq!(installed.host.host_root, config.host.host_root);
        assert_eq!(installed.pins.cargo_tools, config.pins.cargo_tools);
        assert_eq!(installed.pins.brew_formulae, config.pins.brew_formulae);
    }
}
