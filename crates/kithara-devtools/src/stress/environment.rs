#[cfg(test)]
use std::ffi::OsStr;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    path::Path,
    process::Command,
};

use anyhow::{Result, ensure};

use crate::common::project::{StressConfig, StressModeConfig};

#[derive(Debug)]
pub(super) struct CampaignEnvironment {
    set: BTreeMap<OsString, OsString>,
    remove: BTreeSet<OsString>,
}

impl CampaignEnvironment {
    pub(super) fn new(
        raw_dir: &Path,
        config: &StressConfig,
        mode: &StressModeConfig,
    ) -> Result<Self> {
        let remove = config
            .environment
            .remove
            .iter()
            .map(OsString::from)
            .collect::<BTreeSet<_>>();
        let mut set = mode
            .set_env
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect::<BTreeMap<_, _>>();
        for (key, relative) in &mode.raw_path_env {
            ensure!(
                !Path::new(relative).is_absolute(),
                "stress raw environment path for `{key}` must be relative"
            );
            set.insert(OsString::from(key), raw_dir.join(relative).into_os_string());
        }
        Ok(Self { set, remove })
    }

    pub(super) fn apply(&self, command: &mut Command) {
        for key in &self.remove {
            command.env_remove(key);
        }
        command.envs(&self.set);
    }

    #[cfg(test)]
    fn value(&self, key: &str) -> Option<&str> {
        self.set
            .get(OsStr::new(key))
            .and_then(|value| value.to_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::project::{StressEnvironmentConfig, StressModeConfig};

    #[test]
    fn configured_values_and_raw_paths_are_applied_without_product_knowledge() {
        let config = StressConfig {
            environment: StressEnvironmentConfig {
                remove: vec!["OLD_TRACE".to_owned(), "TRACE".to_owned()],
            },
            ..StressConfig::default()
        };
        let mode = StressModeConfig {
            set_env: BTreeMap::from([("TRACE".to_owned(), "verbose".to_owned())]),
            raw_path_env: BTreeMap::from([("DUMP_DIR".to_owned(), "hang".to_owned())]),
            ..StressModeConfig::default()
        };

        let environment =
            CampaignEnvironment::new(Path::new("raw"), &config, &mode).expect("environment");

        assert_eq!(environment.value("TRACE"), Some("verbose"));
        assert_eq!(environment.value("DUMP_DIR"), Some("raw/hang"));
        assert_eq!(environment.value("OLD_TRACE"), None);
    }

    #[test]
    fn absolute_raw_paths_are_rejected_at_the_execution_boundary() {
        let mode = StressModeConfig {
            raw_path_env: BTreeMap::from([("DUMP_DIR".to_owned(), "/tmp/hang".to_owned())]),
            ..StressModeConfig::default()
        };

        let error = CampaignEnvironment::new(Path::new("raw"), &StressConfig::default(), &mode)
            .expect_err("absolute path");

        assert!(error.to_string().contains("must be relative"));
    }
}
