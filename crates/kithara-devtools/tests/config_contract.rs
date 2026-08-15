use std::{fs, path::Path};

use kithara_devtools::common::{
    project::ProjectConfig,
    scope::Scope,
    walker::{relative_to, workspace_rs_files_scoped},
};
use tempfile::tempdir;

fn write_config(root: &Path, text: &str) {
    let config_dir = root.join(".config");
    fs::create_dir_all(&config_dir).expect("create config dir");
    fs::write(config_dir.join("xtask.toml"), text).expect("write config");
}

#[test]
fn missing_config_file_yields_defaults() {
    let temp = tempdir().expect("tempdir");

    let config = ProjectConfig::load(temp.path()).expect("load missing config");

    assert!(config.workspace_scan.exclude.is_empty());
    assert_eq!(config.perf.nextest_profile, "perf");
    assert!(!config.test.no_block.default);
    assert!(config.test.no_block.features.is_empty());
}

#[test]
fn unknown_section_is_a_typed_error() {
    let temp = tempdir().expect("tempdir");
    write_config(
        temp.path(),
        r#"
[not_a_real_section]
name = "typo"
"#,
    );

    let error = ProjectConfig::load(temp.path()).expect_err("unknown section fails");
    let message = format!("{error:#}");

    assert!(
        message.contains("not_a_real_section"),
        "error did not mention offending token: {message}"
    );
}

#[test]
fn top_level_consumer_section_is_rejected() {
    let temp = tempdir().expect("tempdir");
    write_config(
        temp.path(),
        r#"
[android]
ffi_crate = "kithara-ffi"
"#,
    );

    let error = ProjectConfig::load(temp.path()).expect_err("top-level consumer section fails");
    let message = format!("{error:#}");

    assert!(
        message.contains("android"),
        "error did not mention offending token: {message}"
    );
}

#[test]
fn ext_table_accepts_consumer_sections() {
    let temp = tempdir().expect("tempdir");
    write_config(
        temp.path(),
        r#"
[ext.android]
ffi_crate = "kithara-ffi"

[ext.local_tool]
enabled = true
"#,
    );

    let config = ProjectConfig::load(temp.path()).expect("load ext passthrough config");

    assert!(config.ext.contains_key("android"));
    assert!(config.ext.contains_key("local_tool"));
}

#[test]
fn workspace_scan_parses() {
    let temp = tempdir().expect("tempdir");
    write_config(
        temp.path(),
        r#"
[workspace-scan]
exclude = ["foo/**"]
"#,
    );

    let config = ProjectConfig::load(temp.path()).expect("load workspace scan config");

    assert_eq!(config.workspace_scan.exclude, ["foo/**"]);
}

#[test]
fn perf_config_parses_generic_lanes() {
    let temp = tempdir().expect("tempdir");
    write_config(
        temp.path(),
        r#"
[perf]
primary_lane = "flash-on-http"
nextest_profile = "suite-perf"
frame_prefix = "demo"

[[perf.lanes]]
flash = true
backend = "http"

[[perf.lanes]]
flash = false
backend = "native"
"#,
    );

    let config = ProjectConfig::load(temp.path()).expect("load perf config");

    assert_eq!(config.perf.primary_lane, "flash-on-http");
    assert_eq!(config.perf.frame_prefix.as_deref(), Some("demo"));
    assert_eq!(config.perf.nextest_profile, "suite-perf");
    assert_eq!(config.perf.lanes.len(), 2);
    assert!(config.perf.lanes[0].flash);
    assert_eq!(config.perf.lanes[0].backend, "http");
}

#[test]
fn test_config_parses_base_features() {
    let temp = tempdir().expect("tempdir");
    write_config(
        temp.path(),
        r#"
[test]
features = ["always-on"]

[test.flash]
features = ["virtual-time"]

[test.no_block]
default = false
features = ["no-block-detector"]
"#,
    );

    let config = ProjectConfig::load(temp.path()).expect("load test config");

    assert_eq!(config.test.features, ["always-on"]);
    assert_eq!(config.test.flash.features, ["virtual-time"]);
    assert!(!config.test.no_block.default);
    assert_eq!(config.test.no_block.features, ["no-block-detector"]);
}

#[test]
fn stress_config_owns_generic_modes_environment_and_artifacts() {
    let temp = tempdir().expect("tempdir");
    write_config(
        temp.path(),
        r#"
[stress]
default_modes = ["baseline"]
lane = "workspace"
backend = "http"
nextest_config = "config/runner.toml"
nextest_profile = "campaign"
default_filter = "all()"
default_count = 3
max_count = 10
test_threads = "4"
max_test_threads = 8
raw_output = "target/evidence"
report_output = "target/report.md"
workflow_job_timeout_minutes = 60

[stress.artifacts]
subject_junit = "target/runner/junit.xml"
inventory = "inventory.json"
junit = "junit.xml"
log = "runner.log"
manifest = "manifest.json"
pressure = "pressure.jsonl"
report = "report.md"

[stress.environment]
remove = ["OLD_TRACE"]

[stress.modes.baseline]
features = ["snapshot-clock"]

[stress.modes.baseline.set_env]
TRACE_LEVEL = "warn"

[stress.modes.baseline.raw_path_env]
CAPTURE_DIR = "captures"

[test]
default_lane = "workspace"
default_backend = "http"
feature_arg = "--features"

[test.lanes.workspace]
program = "cargo"
prefix_args = ["nextest", "run"]

[test.net_backends.http]
features = []
"#,
    );

    let config = ProjectConfig::load(temp.path()).expect("load stress config");
    let mode = config.stress.mode("baseline").expect("configured mode");

    assert_eq!(config.stress.nextest_profile, "campaign");
    assert_eq!(config.stress.max_count, 10);
    assert_eq!(mode.features, ["snapshot-clock"]);
    assert_eq!(mode.set_env["TRACE_LEVEL"], "warn");
    assert_eq!(mode.raw_path_env["CAPTURE_DIR"], "captures");
}

#[test]
fn stress_envelope_policy_requires_an_envelope_artifact() {
    let temp = tempdir().expect("tempdir");
    write_config(
        temp.path(),
        r#"
[stress]
default_modes = ["baseline"]
lane = "workspace"
backend = "http"
nextest_config = "config/runner.toml"
nextest_profile = "campaign"
default_filter = "all()"
default_count = 1
max_count = 1
test_threads = "1"
max_test_threads = 1
raw_output = "target/evidence"
report_output = "target/report.md"
workflow_job_timeout_minutes = 60

[stress.artifacts]
subject_junit = "target/runner/junit.xml"
inventory = "inventory.json"
junit = "junit.xml"
log = "runner.log"
manifest = "manifest.json"
pressure = "pressure.jsonl"
report = "report.md"

[stress.evidence]
envelope_suffix_markers = [" payload="]

[stress.modes.baseline]

[test]
default_lane = "workspace"
default_backend = "http"

[test.lanes.workspace]
program = "cargo"
prefix_args = ["nextest", "run"]

[test.net_backends.http]
features = []
"#,
    );

    let error = ProjectConfig::load(temp.path()).expect_err("orphan envelope policy fails");

    assert!(format!("{error:#}").contains("stress.artifacts.envelope_dir"));
}

#[test]
fn workspace_scan_excludes_walked_files() {
    let temp = tempdir().expect("tempdir");
    write_config(
        temp.path(),
        r#"
[workspace-scan]
exclude = ["crates/generated/**"]
"#,
    );
    let kept_dir = temp.path().join("crates/app/src");
    let excluded_dir = temp.path().join("crates/generated/src");
    fs::create_dir_all(&kept_dir).expect("create kept dir");
    fs::create_dir_all(&excluded_dir).expect("create excluded dir");
    fs::write(kept_dir.join("lib.rs"), "").expect("write kept file");
    fs::write(excluded_dir.join("lib.rs"), "").expect("write excluded file");

    let files =
        workspace_rs_files_scoped(temp.path(), &Scope::default()).expect("walk scoped files");
    let rels: Vec<String> = files
        .iter()
        .map(|path| {
            relative_to(temp.path(), path)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();

    assert_eq!(rels, ["crates/app/src/lib.rs"]);
}
