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

/// The environment variable every `cargo` invocation reads to decide where it
/// builds, and the one a stress run cannot afford to inherit.
const TARGET_DIR_ENV: &str = "CARGO_TARGET_DIR";

#[derive(Debug)]
pub(super) struct RunEnvironment {
    set: BTreeMap<OsString, OsString>,
    remove: BTreeSet<OsString>,
}

impl RunEnvironment {
    /// # Errors
    ///
    /// Returns an error when a raw path is absolute, or when the build
    /// directory is not one the run can hand a child.
    pub(super) fn new(
        raw_dir: &Path,
        build_dir: &Path,
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
        // The run builds where it says, not where the machine says. An
        // inherited value points at a directory shared with everything else on
        // the host, and a stress run lasts hours: when those binaries went
        // away mid-run, every test of every remaining repeat failed to exec in
        // milliseconds and the lane reported nothing about the code it was
        // asked about. Set last, so no lane can name this key away.
        ensure!(
            build_dir.is_absolute(),
            "stress build directory must be absolute: {}",
            build_dir.display()
        );
        set.insert(
            OsString::from(TARGET_DIR_ENV),
            build_dir.to_path_buf().into_os_string(),
        );
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
            RunEnvironment::new(Path::new("raw"), Path::new("/stress/build"), &config, &mode)
                .expect("environment");

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

        let error = RunEnvironment::new(
            Path::new("raw"),
            Path::new("/stress/build"),
            &StressConfig::default(),
            &mode,
        )
        .expect_err("absolute path");

        assert!(error.to_string().contains("must be relative"));
    }

    #[test]
    fn a_lane_builds_where_the_run_says() {
        let environment = RunEnvironment::new(
            Path::new("raw"),
            Path::new("/stress/build"),
            &StressConfig::default(),
            &StressModeConfig::default(),
        )
        .expect("environment");

        assert_eq!(environment.value(TARGET_DIR_ENV), Some("/stress/build"));
    }

    #[test]
    fn a_lane_cannot_name_the_build_directory_away() {
        let mode = StressModeConfig {
            set_env: BTreeMap::from([(TARGET_DIR_ENV.to_owned(), "/elsewhere".to_owned())]),
            ..StressModeConfig::default()
        };

        let environment = RunEnvironment::new(
            Path::new("raw"),
            Path::new("/stress/build"),
            &StressConfig::default(),
            &mode,
        )
        .expect("environment");

        assert_eq!(environment.value(TARGET_DIR_ENV), Some("/stress/build"));
    }

    #[test]
    fn a_relative_build_directory_is_refused_before_a_child_inherits_it() {
        let error = RunEnvironment::new(
            Path::new("raw"),
            Path::new("build"),
            &StressConfig::default(),
            &StressModeConfig::default(),
        )
        .expect_err("relative build directory");

        assert!(error.to_string().contains("must be absolute"));
    }
}
