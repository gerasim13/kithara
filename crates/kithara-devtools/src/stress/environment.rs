use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    path::Path,
    process::Command,
};

struct Consts;

impl Consts {
    const REPRODUCTION_LOG: &str = "warn";
    const DIAGNOSTIC_LOG: &str = "warn,flash::hang=debug,kithara_platform::no_block=debug,kithara_queue=debug,kithara_hls=debug,kithara_stream=debug,kithara_net=debug,kithara_audio=debug";
    const NO_BLOCK_BUDGET_MS: u64 = 100;
    const PREKILL_SECS: u64 = 630;
    const MANAGED_KEYS: &[&str] = &[
        "KITHARA_FLASH_SYNC_BT",
        "KITHARA_FLASH_SYNC_TRACE",
        "KITHARA_HANG_DUMP_DIR",
        "KITHARA_HANG_PREKILL_SECS",
        "KITHARA_NO_BLOCK",
        "KITHARA_NO_BLOCK_BUDGET_MS",
        "KITHARA_NO_BLOCK_LOG",
        "NEXTEST_FINAL_STATUS_LEVEL",
        "NEXTEST_SHOW_PROGRESS",
        "NEXTEST_STATUS_LEVEL",
        "RUST_BACKTRACE",
        "RUST_LOG",
    ];
}

#[derive(Debug)]
pub(super) struct CampaignEnvironment {
    set: BTreeMap<OsString, OsString>,
    remove: BTreeSet<OsString>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct DiagnosticPolicy {
    pub(super) diagnostics: bool,
    pub(super) dump_thread_backtrace: bool,
    pub(super) no_block: bool,
}

impl CampaignEnvironment {
    pub(super) fn new(raw_dir: &Path, policy: DiagnosticPolicy) -> Self {
        let mut environment = Self {
            set: BTreeMap::new(),
            remove: Consts::MANAGED_KEYS.iter().map(OsString::from).collect(),
        };
        environment.set("RUST_BACKTRACE", "1");
        environment.set("RUST_LOG", log_filter(policy.diagnostics));
        environment.set("NEXTEST_STATUS_LEVEL", "fail");
        environment.set("NEXTEST_FINAL_STATUS_LEVEL", "fail");
        environment.set("NEXTEST_SHOW_PROGRESS", "counter");
        environment.set("KITHARA_HANG_DUMP_DIR", raw_dir.join("hang").as_os_str());
        environment.set(
            "KITHARA_HANG_PREKILL_SECS",
            Consts::PREKILL_SECS.to_string(),
        );
        if policy.diagnostics {
            environment.set("KITHARA_FLASH_SYNC_TRACE", "1");
            if policy.dump_thread_backtrace {
                environment.set("KITHARA_FLASH_SYNC_BT", "1");
            }
            if policy.no_block {
                environment.set("KITHARA_NO_BLOCK", "census");
                environment.set(
                    "KITHARA_NO_BLOCK_BUDGET_MS",
                    Consts::NO_BLOCK_BUDGET_MS.to_string(),
                );
                environment.set(
                    "KITHARA_NO_BLOCK_LOG",
                    raw_dir.join("no-block.log").as_os_str(),
                );
            }
        }
        environment
    }

    pub(super) fn apply(&self, command: &mut Command) {
        for key in &self.remove {
            command.env_remove(key);
        }
        command.envs(&self.set);
    }

    pub(super) fn value(&self, key: &str) -> Option<&str> {
        self.set
            .get(OsStr::new(key))
            .and_then(|value| value.to_str())
    }

    fn set(&mut self, key: impl Into<OsString>, value: impl Into<OsString>) {
        self.set.insert(key.into(), value.into());
    }
}

pub(super) fn log_filter(diagnostics: bool) -> &'static str {
    if diagnostics {
        Consts::DIAGNOSTIC_LOG
    } else {
        Consts::REPRODUCTION_LOG
    }
}

pub(super) const fn no_block_budget_ms() -> u64 {
    Consts::NO_BLOCK_BUDGET_MS
}

pub(super) const fn prekill_secs() -> u64 {
    Consts::PREKILL_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reproduction_removes_timing_perturbing_diagnostics() {
        let environment = CampaignEnvironment::new(
            Path::new("raw"),
            DiagnosticPolicy {
                diagnostics: false,
                dump_thread_backtrace: true,
                no_block: true,
            },
        );

        assert_eq!(
            environment.value("RUST_LOG"),
            Some(Consts::REPRODUCTION_LOG)
        );
        assert_eq!(environment.value("KITHARA_FLASH_SYNC_TRACE"), None);
        assert_eq!(environment.value("KITHARA_FLASH_SYNC_BT"), None);
        assert_eq!(environment.value("KITHARA_NO_BLOCK"), None);
    }

    #[test]
    fn diagnostic_options_are_effective_only_in_diagnostic_mode() {
        let environment = CampaignEnvironment::new(
            Path::new("raw"),
            DiagnosticPolicy {
                diagnostics: true,
                dump_thread_backtrace: true,
                no_block: true,
            },
        );

        assert_eq!(environment.value("RUST_LOG"), Some(Consts::DIAGNOSTIC_LOG));
        assert_eq!(environment.value("KITHARA_FLASH_SYNC_TRACE"), Some("1"));
        assert_eq!(environment.value("KITHARA_FLASH_SYNC_BT"), Some("1"));
        assert_eq!(environment.value("KITHARA_NO_BLOCK"), Some("census"));
        assert_eq!(
            environment.value("KITHARA_NO_BLOCK_LOG"),
            Some("raw/no-block.log")
        );
    }
}
