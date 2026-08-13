use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result, ensure};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use super::{
    environment::{log_filter, no_block_budget_ms, prekill_secs},
    pressure::SCHEMA as PRESSURE_SCHEMA,
    system::SystemSnapshot,
};

mod time;

use time::format_timestamp;

struct Consts;

impl Consts {
    const MANIFEST_SCHEMA: u32 = 2;
    const MAX_MANIFEST_BYTES: usize = 1_048_576;
    const MANIFEST_READ_LIMIT: u64 = 1_048_577;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    pub(super) schema: u32,
    pub(super) mode: Mode,
    pub(super) config: ManifestConfig,
    pub(super) controller: Revision,
    pub(super) subject: Revision,
    pub(super) run: RunMetadata,
    pub(super) selection: Selection,
    pub(super) features: Features,
    pub(super) logging: Logging,
    pub(super) timing: Timing,
    pub(super) pressure: Pressure,
    pub(super) system: SystemSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ManifestSpec {
    pub(super) mode: Mode,
    pub(super) config: ManifestConfig,
    pub(super) controller_sha: String,
    pub(super) subject_sha: String,
    pub(super) run: RunMetadata,
    pub(super) selection: Selection,
    pub(super) features: Features,
    pub(super) logging: Logging,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(super) enum Mode {
    Reproduction,
    Diagnostic,
}

impl Mode {
    #[must_use]
    pub(super) const fn from_diagnostics(enabled: bool) -> Self {
        if enabled {
            Self::Diagnostic
        } else {
            Self::Reproduction
        }
    }

    #[must_use]
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Reproduction => "reproduction",
            Self::Diagnostic => "diagnostic",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestConfig {
    pub(super) profile: String,
    pub(super) nextest: String,
    pub(super) pressure_schema: String,
    pub(super) workflow_job_timeout_minutes: u64,
}

impl ManifestConfig {
    #[must_use]
    pub(super) fn stress(nextest: impl Into<String>, workflow_job_timeout_minutes: u64) -> Self {
        Self {
            profile: "stress".to_owned(),
            nextest: nextest.into(),
            pressure_schema: PRESSURE_SCHEMA.to_owned(),
            workflow_job_timeout_minutes,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Revision {
    pub(super) sha: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RunMetadata {
    pub(super) repository: Option<String>,
    pub(super) workflow: Option<String>,
    pub(super) job: Option<String>,
    pub(super) event: Option<String>,
    pub(super) run_id: Option<String>,
    pub(super) run_attempt: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Selection {
    pub(super) filter: String,
    pub(super) count: usize,
    pub(super) test_threads: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Features {
    pub(super) flash: bool,
    pub(super) diagnostics: bool,
    pub(super) no_block_requested: bool,
    pub(super) no_block: bool,
    pub(super) dump_thread_backtrace_requested: bool,
    pub(super) dump_thread_backtrace: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Logging {
    pub(super) rust_backtrace: String,
    pub(super) rust_log: String,
    pub(super) flash_sync_trace: bool,
    pub(super) flash_dump_thread_backtrace: bool,
    pub(super) no_block_mode: String,
    pub(super) no_block_budget_ms: Option<u64>,
    pub(super) no_block_log: Option<String>,
    pub(super) hang_dump_dir: String,
    pub(super) hang_prekill_secs: Option<u64>,
    pub(super) nextest_status_level: String,
    pub(super) nextest_final_status_level: String,
    pub(super) nextest_show_progress: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Timing {
    pub(super) started_at: String,
    pub(super) ended_at: Option<String>,
    pub(super) exit_code: Option<i32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Pressure {
    pub(super) sampler_healthy: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ExpectedProvenance {
    pub(super) controller_sha: String,
    pub(super) subject_sha: String,
    pub(super) filter: String,
    pub(super) count: usize,
    pub(super) test_threads: String,
    pub(super) mode: Mode,
    pub(super) flash: bool,
    pub(super) no_block_requested: bool,
    pub(super) dump_thread_backtrace_requested: bool,
    pub(super) workflow_job_timeout_minutes: u64,
    pub(super) execute_result: ExecuteResult,
    pub(super) sampler_healthy: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(super) enum ExecuteResult {
    Success,
    Failure,
    Cancelled,
}

impl ExecuteResult {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ProvenanceMismatch {
    pub(super) field: &'static str,
    pub(super) actual: String,
    pub(super) expected: String,
}

impl ProvenanceMismatch {
    fn new(field: &'static str, actual: impl Into<String>, expected: impl Into<String>) -> Self {
        Self {
            field,
            actual: actual.into(),
            expected: expected.into(),
        }
    }
}

impl fmt::Display for ProvenanceMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}={}, expected {}",
            self.field, self.actual, self.expected
        )
    }
}

impl Manifest {
    pub(super) fn start(spec: ManifestSpec, system: SystemSnapshot) -> Result<Self> {
        let started_at = format_timestamp(SystemTime::now())?;
        let manifest = Self {
            schema: Consts::MANIFEST_SCHEMA,
            mode: spec.mode,
            config: spec.config,
            controller: Revision {
                sha: spec.controller_sha,
            },
            subject: Revision {
                sha: spec.subject_sha,
            },
            run: spec.run,
            selection: spec.selection,
            features: spec.features,
            logging: spec.logging,
            timing: Timing {
                started_at,
                ended_at: None,
                exit_code: None,
            },
            pressure: Pressure {
                sampler_healthy: None,
            },
            system,
        };
        manifest.validate_invariants()?;
        Ok(manifest)
    }

    pub(super) fn finalize(&mut self, exit_code: i32, sampler_healthy: bool) -> Result<()> {
        ensure!(
            self.timing.ended_at.is_none() && self.timing.exit_code.is_none(),
            "stress manifest is already finalized"
        );
        self.timing.ended_at = Some(format_timestamp(SystemTime::now())?);
        self.timing.exit_code = Some(exit_code);
        self.pressure.sampler_healthy = Some(sampler_healthy);
        Ok(())
    }

    pub(super) fn write_atomic(&self, path: &Path) -> Result<()> {
        let mut contents = serde_json::to_vec_pretty(self).context("serialize stress manifest")?;
        contents.push(b'\n');
        ensure!(
            contents.len() <= Consts::MAX_MANIFEST_BYTES,
            "stress manifest is {} bytes; maximum is {}",
            contents.len(),
            Consts::MAX_MANIFEST_BYTES,
        );
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).with_context(|| {
                format!("create stress manifest directory {}", parent.display())
            })?;
        }

        let temporary = temporary_path(path)?;
        let publication = write_and_publish(&temporary, path, &contents);
        if publication.is_err() {
            let _cleanup = fs::remove_file(&temporary);
        }
        publication
    }

    pub(super) fn read(path: &Path) -> Result<Self> {
        let file =
            File::open(path).with_context(|| format!("open stress manifest {}", path.display()))?;
        let mut contents = Vec::new();
        file.take(Consts::MANIFEST_READ_LIMIT)
            .read_to_end(&mut contents)
            .with_context(|| format!("read stress manifest {}", path.display()))?;
        ensure!(
            !contents.is_empty(),
            "stress manifest is empty: {}",
            path.display()
        );
        ensure!(
            contents.len() <= Consts::MAX_MANIFEST_BYTES,
            "stress manifest exceeds {} bytes: {}",
            Consts::MAX_MANIFEST_BYTES,
            path.display()
        );
        let manifest: Self = serde_json::from_slice(&contents)
            .with_context(|| format!("parse stress manifest {}", path.display()))?;
        ensure!(
            manifest.schema == Consts::MANIFEST_SCHEMA,
            "stress manifest schema is {}, expected {}",
            manifest.schema,
            Consts::MANIFEST_SCHEMA,
        );
        manifest.validate_invariants()?;
        Ok(manifest)
    }

    #[must_use]
    pub(super) fn validate_provenance(
        &self,
        expected: &ExpectedProvenance,
    ) -> Vec<ProvenanceMismatch> {
        let mut mismatches = Vec::new();
        self.validate_identity(expected, &mut mismatches);
        self.validate_selection(expected, &mut mismatches);
        self.validate_features(expected, &mut mismatches);
        self.validate_timing(expected, &mut mismatches);
        self.validate_pressure(expected, &mut mismatches);
        mismatches
    }

    fn validate_identity(
        &self,
        expected: &ExpectedProvenance,
        mismatches: &mut Vec<ProvenanceMismatch>,
    ) {
        if self.schema != Consts::MANIFEST_SCHEMA {
            mismatches.push(ProvenanceMismatch::new(
                "schema",
                self.schema.to_string(),
                Consts::MANIFEST_SCHEMA.to_string(),
            ));
        }
        compare_sha(
            "controller.sha",
            &self.controller.sha,
            &expected.controller_sha,
            mismatches,
        );
        compare_sha(
            "subject.sha",
            &self.subject.sha,
            &expected.subject_sha,
            mismatches,
        );
        if self.mode != expected.mode {
            mismatches.push(ProvenanceMismatch::new(
                "mode",
                quoted(self.mode.as_str()),
                quoted(expected.mode.as_str()),
            ));
        }
    }

    fn validate_selection(
        &self,
        expected: &ExpectedProvenance,
        mismatches: &mut Vec<ProvenanceMismatch>,
    ) {
        compare_string(
            "selection.filter",
            &self.selection.filter,
            &expected.filter,
            mismatches,
        );
        if self.selection.count != expected.count {
            mismatches.push(ProvenanceMismatch::new(
                "selection.count",
                self.selection.count.to_string(),
                expected.count.to_string(),
            ));
        }
        compare_string(
            "selection.test_threads",
            &self.selection.test_threads,
            &expected.test_threads,
            mismatches,
        );
        if self.config.workflow_job_timeout_minutes != expected.workflow_job_timeout_minutes {
            mismatches.push(ProvenanceMismatch::new(
                "config.workflow_job_timeout_minutes",
                self.config.workflow_job_timeout_minutes.to_string(),
                expected.workflow_job_timeout_minutes.to_string(),
            ));
        }
    }

    fn validate_timing(
        &self,
        expected: &ExpectedProvenance,
        mismatches: &mut Vec<ProvenanceMismatch>,
    ) {
        if self.timing.started_at.trim().is_empty() {
            mismatches.push(ProvenanceMismatch::new(
                "timing.started_at",
                quoted(&self.timing.started_at),
                "a non-empty timestamp",
            ));
        }
        match self.timing.ended_at.as_deref() {
            Some(value) if !value.trim().is_empty() => {}
            value => mismatches.push(ProvenanceMismatch::new(
                "timing.ended_at",
                optional_quoted(value),
                "a finalized non-empty timestamp",
            )),
        }
        let Some(exit_code) = self.timing.exit_code else {
            mismatches.push(ProvenanceMismatch::new(
                "timing.exit_code",
                "null",
                "a finalized integer exit code",
            ));
            return;
        };
        let result_matches = match expected.execute_result {
            ExecuteResult::Success => exit_code == 0,
            ExecuteResult::Failure | ExecuteResult::Cancelled => exit_code != 0,
        };
        if !result_matches {
            let expectation = match expected.execute_result {
                ExecuteResult::Success => "zero for a successful execute job",
                ExecuteResult::Failure => "nonzero for a failed execute job",
                ExecuteResult::Cancelled => "nonzero for a cancelled execute job",
            };
            mismatches.push(ProvenanceMismatch::new(
                "timing.exit_code",
                format!(
                    "{exit_code} with execute result {}",
                    expected.execute_result.as_str()
                ),
                expectation,
            ));
        }
    }

    fn validate_features(
        &self,
        expected: &ExpectedProvenance,
        mismatches: &mut Vec<ProvenanceMismatch>,
    ) {
        compare_bool(
            "features.flash",
            self.features.flash,
            expected.flash,
            mismatches,
        );
        compare_bool(
            "features.no_block_requested",
            self.features.no_block_requested,
            expected.no_block_requested,
            mismatches,
        );
        compare_bool(
            "features.dump_thread_backtrace_requested",
            self.features.dump_thread_backtrace_requested,
            expected.dump_thread_backtrace_requested,
            mismatches,
        );
        let diagnostic = expected.mode == Mode::Diagnostic;
        compare_bool(
            "features.diagnostics",
            self.features.diagnostics,
            diagnostic,
            mismatches,
        );
        compare_bool(
            "features.no_block",
            self.features.no_block,
            diagnostic && expected.no_block_requested,
            mismatches,
        );
        compare_bool(
            "features.dump_thread_backtrace",
            self.features.dump_thread_backtrace,
            diagnostic && expected.dump_thread_backtrace_requested,
            mismatches,
        );
    }

    fn validate_invariants(&self) -> Result<()> {
        ensure!(
            valid_sha(&self.controller.sha),
            "manifest controller SHA is invalid"
        );
        ensure!(
            valid_sha(&self.subject.sha),
            "manifest subject SHA is invalid"
        );
        ensure!(
            !self.selection.filter.trim().is_empty(),
            "manifest filter is empty"
        );
        ensure!(self.selection.count > 0, "manifest count is zero");
        ensure!(
            !self.selection.test_threads.trim().is_empty(),
            "manifest test-threads value is empty"
        );
        ensure!(
            self.config.profile == "stress",
            "manifest profile is not `stress`"
        );
        ensure!(
            !self.config.nextest.trim().is_empty(),
            "manifest nextest configuration path is empty"
        );
        ensure!(
            self.config.pressure_schema == PRESSURE_SCHEMA,
            "manifest pressure schema is invalid"
        );
        ensure!(
            self.config.workflow_job_timeout_minutes > 0,
            "manifest job timeout is zero"
        );
        let diagnostic = self.mode == Mode::Diagnostic;
        ensure!(
            self.features.diagnostics == diagnostic,
            "manifest mode and diagnostics disagree"
        );
        ensure!(
            self.features.no_block == (diagnostic && self.features.no_block_requested),
            "manifest no-block request and effective value disagree"
        );
        ensure!(
            self.features.dump_thread_backtrace
                == (diagnostic && self.features.dump_thread_backtrace_requested),
            "manifest dump-backtrace request and effective value disagree"
        );
        ensure!(
            self.logging.flash_sync_trace == diagnostic,
            "manifest Flash trace setting contradicts diagnostic mode"
        );
        ensure!(
            self.logging.flash_dump_thread_backtrace == self.features.dump_thread_backtrace,
            "manifest Flash dump-backtrace setting contradicts effective features"
        );
        ensure!(
            self.logging.rust_backtrace == "1",
            "manifest Rust backtrace setting is invalid"
        );
        ensure!(
            self.logging.rust_log == log_filter(diagnostic),
            "manifest Rust log setting contradicts diagnostic mode"
        );
        let no_block = self.features.no_block;
        let expected_no_block_mode = if no_block { "census" } else { "off" };
        ensure!(
            self.logging.no_block_mode == expected_no_block_mode,
            "manifest no-block mode contradicts effective features"
        );
        let expected_no_block_budget = no_block.then_some(no_block_budget_ms());
        ensure!(
            self.logging.no_block_budget_ms == expected_no_block_budget,
            "manifest no-block budget contradicts effective features"
        );
        ensure!(
            self.logging
                .no_block_log
                .as_deref()
                .is_some_and(|path| !path.is_empty())
                == no_block,
            "manifest no-block log contradicts effective features"
        );
        ensure!(
            !self.logging.hang_dump_dir.is_empty(),
            "manifest hang dump directory is empty"
        );
        ensure!(
            self.logging.hang_prekill_secs == Some(prekill_secs()),
            "manifest hang pre-kill setting is invalid"
        );
        ensure!(
            self.logging.nextest_status_level == "fail"
                && self.logging.nextest_final_status_level == "fail"
                && self.logging.nextest_show_progress == "counter",
            "manifest nextest output settings are invalid"
        );
        Ok(())
    }

    fn validate_pressure(
        &self,
        expected: &ExpectedProvenance,
        mismatches: &mut Vec<ProvenanceMismatch>,
    ) {
        if self.pressure.sampler_healthy != Some(expected.sampler_healthy) {
            mismatches.push(ProvenanceMismatch::new(
                "pressure.sampler_healthy",
                optional_bool(self.pressure.sampler_healthy),
                expected.sampler_healthy.to_string(),
            ));
        }
    }
}

fn write_and_publish(temporary: &Path, destination: &Path, contents: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary)
        .with_context(|| format!("create temporary stress manifest {}", temporary.display()))?;
    file.write_all(contents)
        .with_context(|| format!("write temporary stress manifest {}", temporary.display()))?;
    file.sync_all()
        .with_context(|| format!("sync temporary stress manifest {}", temporary.display()))?;
    drop(file);
    fs::rename(temporary, destination).with_context(|| {
        format!(
            "publish stress manifest {} as {}",
            temporary.display(),
            destination.display()
        )
    })
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let file_name = path
        .file_name()
        .with_context(|| format!("stress manifest path has no file name: {}", path.display()))?;
    let mut temporary_name = file_name.to_os_string();
    temporary_name.push(format!(".{}.tmp", std::process::id()));
    Ok(path.with_file_name(temporary_name))
}

fn compare_sha(
    field: &'static str,
    actual: &str,
    expected: &str,
    mismatches: &mut Vec<ProvenanceMismatch>,
) {
    if !actual.eq_ignore_ascii_case(expected) {
        mismatches.push(ProvenanceMismatch::new(
            field,
            quoted(actual),
            quoted(expected),
        ));
    }
}

fn compare_string(
    field: &'static str,
    actual: &str,
    expected: &str,
    mismatches: &mut Vec<ProvenanceMismatch>,
) {
    if actual != expected {
        mismatches.push(ProvenanceMismatch::new(
            field,
            quoted(actual),
            quoted(expected),
        ));
    }
}

fn compare_bool(
    field: &'static str,
    actual: bool,
    expected: bool,
    mismatches: &mut Vec<ProvenanceMismatch>,
) {
    if actual != expected {
        mismatches.push(ProvenanceMismatch::new(
            field,
            actual.to_string(),
            expected.to_string(),
        ));
    }
}

fn valid_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn quoted(value: &str) -> String {
    format!("{value:?}")
}

fn optional_quoted(value: Option<&str>) -> String {
    value.map_or_else(|| "null".to_owned(), quoted)
}

fn optional_bool(value: Option<bool>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stress::system::{CgroupScope, CgroupV2, CpuSet, Limits};

    const CONTROLLER_SHA: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    const SUBJECT_SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn system() -> SystemSnapshot {
        SystemSnapshot {
            kernel: "Linux 6.12-test #1 x86_64".to_owned(),
            cgroup_v2: CgroupV2 {
                scope: CgroupScope::CurrentProcessCgroup,
                path: Some(PathBuf::from("/sys/fs/cgroup/job.scope")),
            },
            cpuset: CpuSet {
                cgroup_effective: Some("0-3".to_owned()),
                proc_allowed: Some("0-3".to_owned()),
            },
            limits: Limits {
                cgroup_cpu_max: Some("max 100000".to_owned()),
                cgroup_memory_max: None,
                cgroup_pids_max: Some("512".to_owned()),
                ulimit_open_files: Some("1024".to_owned()),
                ulimit_processes: Some("4096".to_owned()),
            },
        }
    }

    fn logging() -> Logging {
        Logging {
            rust_backtrace: "1".to_owned(),
            rust_log: "warn".to_owned(),
            flash_sync_trace: false,
            flash_dump_thread_backtrace: false,
            no_block_mode: "off".to_owned(),
            no_block_budget_ms: None,
            no_block_log: None,
            hang_dump_dir: "raw/hang".to_owned(),
            hang_prekill_secs: Some(630),
            nextest_status_level: "fail".to_owned(),
            nextest_final_status_level: "fail".to_owned(),
            nextest_show_progress: "counter".to_owned(),
        }
    }

    fn spec() -> ManifestSpec {
        ManifestSpec {
            mode: Mode::Reproduction,
            config: ManifestConfig::stress("controller/.config/nextest.toml", 1_380),
            controller_sha: CONTROLLER_SHA.to_owned(),
            subject_sha: SUBJECT_SHA.to_owned(),
            run: RunMetadata::default(),
            selection: Selection {
                filter: "all()".to_owned(),
                count: 50,
                test_threads: "num-cpus".to_owned(),
            },
            features: Features::default(),
            logging: logging(),
        }
    }

    fn expected() -> ExpectedProvenance {
        ExpectedProvenance {
            controller_sha: CONTROLLER_SHA.to_ascii_lowercase(),
            subject_sha: SUBJECT_SHA.to_ascii_uppercase(),
            filter: "all()".to_owned(),
            count: 50,
            test_threads: "num-cpus".to_owned(),
            mode: Mode::Reproduction,
            flash: false,
            no_block_requested: false,
            dump_thread_backtrace_requested: false,
            workflow_job_timeout_minutes: 1_380,
            execute_result: ExecuteResult::Success,
            sampler_healthy: true,
        }
    }

    #[test]
    fn finalized_manifest_round_trips_through_atomic_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("raw/manifest.json");
        let mut manifest = Manifest::start(spec(), system()).expect("start manifest");
        manifest.finalize(0, true).expect("finalize manifest");

        manifest.write_atomic(&path).expect("write manifest");
        let parsed = Manifest::read(&path).expect("read manifest");

        assert_eq!(parsed, manifest);
        assert_eq!(parsed.validate_provenance(&expected()), Vec::new());
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(path).expect("read JSON")).expect("parse JSON");
        assert_eq!(json["schema"], Consts::MANIFEST_SCHEMA);
        assert_eq!(json["selection"]["count"], 50);
        assert_eq!(json["pressure"]["sampler_healthy"], true);
    }

    #[test]
    fn provenance_validation_accumulates_actionable_mismatches() {
        let manifest = Manifest::start(spec(), system()).expect("start manifest");
        let mut expected = expected();
        expected.subject_sha = "cccccccccccccccccccccccccccccccccccccccc".to_owned();
        expected.filter = "package(foo)".to_owned();
        expected.count = 100;
        expected.test_threads = "1".to_owned();
        expected.mode = Mode::Diagnostic;
        expected.flash = true;
        expected.no_block_requested = true;
        expected.dump_thread_backtrace_requested = true;
        expected.workflow_job_timeout_minutes = 60;

        let mismatches = manifest.validate_provenance(&expected);
        let fields = mismatches
            .iter()
            .map(|mismatch| mismatch.field)
            .collect::<Vec<_>>();

        assert!(!fields.contains(&"controller.sha"));
        for field in [
            "subject.sha",
            "mode",
            "features.flash",
            "features.diagnostics",
            "features.no_block_requested",
            "features.no_block",
            "features.dump_thread_backtrace_requested",
            "features.dump_thread_backtrace",
            "selection.filter",
            "selection.count",
            "selection.test_threads",
            "config.workflow_job_timeout_minutes",
            "timing.ended_at",
            "timing.exit_code",
            "pressure.sampler_healthy",
        ] {
            assert!(fields.contains(&field), "missing mismatch for {field}");
        }
        assert!(
            mismatches
                .iter()
                .all(|mismatch| mismatch.to_string().contains("expected"))
        );
    }

    #[test]
    fn execute_result_must_agree_with_final_exit_code() {
        let mut manifest = Manifest::start(spec(), system()).expect("start manifest");
        manifest.finalize(17, true).expect("finalize manifest");

        let success_mismatches = manifest.validate_provenance(&expected());
        assert!(
            success_mismatches
                .iter()
                .any(|mismatch| mismatch.field == "timing.exit_code")
        );

        for execute_result in [ExecuteResult::Failure, ExecuteResult::Cancelled] {
            let mut expected = expected();
            expected.execute_result = execute_result;
            assert_eq!(manifest.validate_provenance(&expected), Vec::new());
        }
    }

    #[test]
    fn bounded_reader_rejects_an_oversized_manifest() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("manifest.json");
        fs::write(&path, vec![b' '; Consts::MAX_MANIFEST_BYTES + 1])
            .expect("write oversized fixture");

        let error = Manifest::read(&path).expect_err("oversized manifest must fail");

        assert!(error.to_string().contains("exceeds"), "{error:#}");
    }

    #[test]
    fn reader_rejects_a_tampered_empty_nextest_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("manifest.json");
        let mut manifest = Manifest::start(spec(), system()).expect("start manifest");
        manifest.finalize(0, true).expect("finalize manifest");
        let mut json = serde_json::to_value(manifest).expect("serialize manifest");
        json["config"]["nextest"] = serde_json::Value::String(String::new());
        fs::write(&path, serde_json::to_vec(&json).expect("encode manifest"))
            .expect("write manifest");

        let error = Manifest::read(&path).expect_err("empty nextest path must fail");

        assert!(error.to_string().contains("nextest configuration"));
    }

    #[test]
    fn reader_rejects_logging_that_contradicts_reproduction_mode() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("manifest.json");
        let manifest = Manifest::start(spec(), system()).expect("start manifest");
        let mut json = serde_json::to_value(manifest).expect("serialize manifest");
        json["logging"]["flash_sync_trace"] = serde_json::Value::Bool(true);
        fs::write(&path, serde_json::to_vec(&json).expect("encode manifest"))
            .expect("write manifest");

        let error = Manifest::read(&path).expect_err("contradictory logging must fail");

        assert!(error.to_string().contains("contradicts diagnostic mode"));
    }

    #[test]
    fn bounded_writer_does_not_publish_an_oversized_manifest() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("manifest.json");
        let mut spec = spec();
        spec.selection.filter = "x".repeat(Consts::MAX_MANIFEST_BYTES);
        let manifest = Manifest::start(spec, system()).expect("start manifest");

        let error = manifest
            .write_atomic(&path)
            .expect_err("oversized manifest must fail");

        assert!(error.to_string().contains("maximum"), "{error:#}");
        assert!(!path.exists());
    }

    #[test]
    fn rejects_an_unrecognized_schema_after_bounded_parse() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("manifest.json");
        let manifest = Manifest::start(spec(), system()).expect("start manifest");
        let mut json = serde_json::to_value(manifest).expect("serialize manifest");
        json["schema"] = serde_json::json!(Consts::MANIFEST_SCHEMA + 1);
        fs::write(&path, serde_json::to_vec(&json).expect("encode fixture"))
            .expect("write fixture");

        let error = Manifest::read(&path).expect_err("unknown schema must fail");

        assert!(error.to_string().contains("schema"), "{error:#}");
    }
}
