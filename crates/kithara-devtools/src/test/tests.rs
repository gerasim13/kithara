use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::Command,
};

use tempfile::TempDir;

use super::{
    command::{lane_command, nextest_lane_command_for, run_each, run_lane},
    request::TestRequest,
    selection::{features_for, select_lane, validate_config},
    *,
};
use crate::{
    common::project::{
        AuditClippyConfig, HealthConfig, KnownFlake, LintExcludeConfig, OrphansConfig, PerfConfig,
        ProjectConfig, ProjectIdentity, QualityConfig, StressConfig, TestCommandConfig,
        TestFlashConfig, TestLaneConfig, TestNetBackendConfig, TestNoBlockConfig, WorkspaceScan,
    },
    consts,
    verdict::ChildFailure,
};

pub(super) fn args_of(cmd: &Command) -> Vec<String> {
    cmd.get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

pub(super) fn envs_of(cmd: &Command) -> Vec<(String, String)> {
    cmd.get_envs()
        .filter_map(|(key, value)| {
            value.map(|value| {
                (
                    key.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
        })
        .collect()
}

fn synthetic_project() -> ProjectConfig {
    let mut lanes = BTreeMap::new();
    lanes.insert(
        "workspace".to_owned(),
        TestLaneConfig {
            program: "cargo".to_owned(),
            prefix_args: vec![
                "nextest".to_owned(),
                "run".to_owned(),
                "--workspace".to_owned(),
            ],
            suffix_args: vec!["--locked".to_owned()],
            default_features: Vec::new(),
            default_backend: None,
            default_flash: None,
            default_no_block: None,
            undeclared_toggles: Vec::new(),
            passthrough: String::new(),
            env: BTreeMap::new(),
            owns: Vec::new(),
        },
    );
    lanes.insert(
        "loom".to_owned(),
        TestLaneConfig {
            program: "cargo".to_owned(),
            prefix_args: vec![
                "nextest".to_owned(),
                "run".to_owned(),
                "--workspace".to_owned(),
            ],
            suffix_args: vec!["-E".to_owned(), "test(loom_model_)".to_owned()],
            default_features: vec!["demo/loom".to_owned()],
            default_backend: None,
            default_flash: Some(false),
            default_no_block: None,
            undeclared_toggles: Vec::new(),
            passthrough: String::new(),
            env: BTreeMap::new(),
            owns: Vec::new(),
        },
    );
    lanes.insert(
        "toolsmith".to_owned(),
        TestLaneConfig {
            program: "cargo".to_owned(),
            prefix_args: vec!["nextest".to_owned(), "run".to_owned()],
            suffix_args: Vec::new(),
            default_features: Vec::new(),
            default_backend: None,
            default_flash: None,
            default_no_block: None,
            undeclared_toggles: vec![
                consts::FLASH_TOGGLE.to_owned(),
                consts::NO_BLOCK_TOGGLE.to_owned(),
            ],
            passthrough: String::new(),
            env: BTreeMap::new(),
            owns: Vec::new(),
        },
    );
    lanes.insert(
        "detector".to_owned(),
        TestLaneConfig {
            program: "cargo".to_owned(),
            prefix_args: vec!["nextest".to_owned(), "run".to_owned()],
            suffix_args: Vec::new(),
            default_features: Vec::new(),
            default_backend: None,
            default_flash: None,
            default_no_block: Some(true),
            undeclared_toggles: Vec::new(),
            passthrough: String::new(),
            env: BTreeMap::new(),
            owns: Vec::new(),
        },
    );
    lanes.insert(
        "browser".to_owned(),
        TestLaneConfig {
            program: "cargo".to_owned(),
            prefix_args: vec!["test".to_owned()],
            suffix_args: vec!["selenium".to_owned()],
            default_features: Vec::new(),
            default_backend: None,
            default_flash: Some(false),
            default_no_block: None,
            undeclared_toggles: Vec::new(),
            passthrough: "after-suffix".to_owned(),
            env: BTreeMap::from([("DEMO_BROWSER".to_owned(), "firefox".to_owned())]),
            owns: Vec::new(),
        },
    );
    let mut net_backends = BTreeMap::new();
    net_backends.insert(
        "http".to_owned(),
        TestNetBackendConfig {
            features: Vec::new(),
        },
    );
    net_backends.insert(
        "native".to_owned(),
        TestNetBackendConfig {
            features: vec!["demo/native-net".to_owned()],
        },
    );
    ProjectConfig {
        architecture: crate::common::project::ArchitectureConfig::default(),
        project: ProjectIdentity {
            name: "demo".to_owned(),
        },
        audit_clippy: AuditClippyConfig::default(),
        ci_report: crate::common::project::CiReportConfig::default(),
        health: HealthConfig::default(),
        test: TestCommandConfig {
            lanes,
            net_backends,
            shared_paths: Vec::new(),
            default_lane: "workspace".to_owned(),
            default_backend: "http".to_owned(),
            feature_arg: "--features".to_owned(),
            nextest_config: ".config/nextest.toml".to_owned(),
            known_flakes: Vec::new(),
            features: vec!["base-feature".to_owned()],
            flash: TestFlashConfig {
                features: vec!["virtual-time".to_owned()],
                default: true,
            },
            no_block: TestNoBlockConfig {
                features: vec!["nb-detect".to_owned()],
                default: false,
            },
            loom_lane: "loom".to_owned(),
        },
        lint_exclude: LintExcludeConfig::default(),
        workspace_scan: WorkspaceScan::default(),
        orphans: OrphansConfig::default(),
        quality: QualityConfig::default(),
        perf: PerfConfig::default(),
        stress: StressConfig::default(),
        ext: toml::Table::default(),
        tools: crate::common::tools::ToolsConfig::default(),
    }
}

#[test]
fn lane_backend_default_does_not_override_an_explicit_request() {
    let mut project = synthetic_project();
    project.test.default_backend = "native".into();
    project
        .test
        .lanes
        .get_mut("workspace")
        .unwrap()
        .default_backend = Some("http".into());
    let default = TestRequest::parse(&["--flash=off".into()]).unwrap();
    let lane = &project.test.lanes["workspace"];
    let features = features_for(&project.test, lane, &default).unwrap();
    let explicit =
        TestRequest::parse(&["--flash=off".into(), "--net-backend=native".into()]).unwrap();
    let requested = features_for(&project.test, lane, &explicit).unwrap();
    assert_eq!(features, BTreeSet::from(["base-feature".into()]));
    assert_eq!(
        requested,
        BTreeSet::from(["base-feature".into(), "demo/native-net".into()])
    );
}

#[test]
fn lane_features_flash_and_backend() {
    let project = synthetic_project();
    let test = &project.test;
    let lane = &test.lanes[&test.default_lane];

    let feats = lane_features(
        test,
        lane,
        LaneToggles {
            flash: true,
            no_block: false,
        },
        "native",
    )
    .expect("features");
    assert!(feats.contains("base-feature"));
    assert!(feats.contains("virtual-time"));
    assert!(feats.contains("demo/native-net"));
    assert!(!feats.contains("nb-detect"));

    let feats = lane_features(
        test,
        lane,
        LaneToggles {
            flash: false,
            no_block: false,
        },
        "http",
    )
    .expect("features");
    assert_eq!(feats, BTreeSet::from(["base-feature".to_owned()]));
}

#[test]
fn features_default_request_omits_no_block() {
    let project = synthetic_project();
    let test = &project.test;
    let lane = &test.lanes[&test.default_lane];
    let request = TestRequest::parse(&[]).expect("parse request");

    let feats = features_for(test, lane, &request).expect("features");
    assert!(!feats.contains("nb-detect"));
}

#[test]
fn default_lane_command_resolves_cli_then_lane_then_project_backend() {
    let mut project = synthetic_project();
    project
        .test
        .lanes
        .get_mut("workspace")
        .expect("workspace")
        .default_backend = Some("native".to_owned());
    let lane = &project.test.lanes[&project.test.default_lane];

    let lane_backend = lane_command(
        &project,
        &project.test.default_lane,
        lane,
        &TestRequest::parse(&[]).expect("parse request"),
    )
    .expect("default lane command");
    assert!(
        args_of(&lane_backend)
            .iter()
            .any(|arg| arg.contains("demo/native-net"))
    );

    let cli_backend = lane_command(
        &project,
        &project.test.default_lane,
        lane,
        &TestRequest::parse(&["--net-backend=http".to_owned()]).expect("parse request"),
    )
    .expect("default lane command");
    assert!(
        !args_of(&cli_backend)
            .iter()
            .any(|arg| arg.contains("demo/native-net"))
    );
}

#[test]
fn lane_backend_must_be_configured() {
    let mut project = synthetic_project();
    project
        .test
        .lanes
        .get_mut("browser")
        .expect("browser")
        .default_backend = Some("missing".to_owned());

    let error = validate_config(&project.test).expect_err("unknown lane backend fails");

    assert!(
        error
            .to_string()
            .contains("test.lanes.browser.default_backend")
    );
}

#[test]
fn no_block_on_adds_features_and_composes_with_flash_and_backend() {
    let project = synthetic_project();
    let test = &project.test;
    let lane = &test.lanes[&test.default_lane];
    let request = TestRequest::parse(&[
        "--flash=on".to_owned(),
        "--no-block=on".to_owned(),
        "--net-backend=native".to_owned(),
    ])
    .expect("parse request");

    let feats = features_for(test, lane, &request).expect("features");
    assert!(feats.contains("base-feature"));
    assert!(feats.contains("virtual-time"));
    assert!(feats.contains("demo/native-net"));
    assert!(feats.contains("nb-detect"));
}

#[test]
fn no_block_off_keeps_no_block_features_out() {
    let project = synthetic_project();
    let test = &project.test;
    let lane = &test.lanes[&test.default_lane];
    let request = TestRequest::parse(&[
        "--flash=on".to_owned(),
        "--no-block=off".to_owned(),
        "--net-backend=native".to_owned(),
    ])
    .expect("parse request");

    let feats = features_for(test, lane, &request).expect("features");
    assert!(!feats.contains("nb-detect"));
    assert!(feats.contains("virtual-time"));
}

#[test]
fn a_lane_that_asks_for_the_detector_gets_it_without_a_flag() {
    let project = synthetic_project();
    let test = &project.test;
    let lane = &test.lanes["detector"];
    let request = TestRequest::parse(&[]).expect("parse request");

    let feats = features_for(test, lane, &request).expect("features");

    assert!(feats.contains("nb-detect"));
}

#[test]
fn an_explicit_off_overrides_the_lane_detector_default() {
    let project = synthetic_project();
    let test = &project.test;
    let lane = &test.lanes["detector"];
    let request = TestRequest::parse(&["--no-block=off".to_owned()]).expect("parse request");

    let feats = features_for(test, lane, &request).expect("features");

    assert!(!feats.contains("nb-detect"));
}

#[test]
fn a_lane_without_the_detector_stays_without_it_when_the_gate_asks_for_it() {
    let project = synthetic_project();
    let test = &project.test;
    let lane = &test.lanes["toolsmith"];
    let request = TestRequest::parse(&["--no-block=on".to_owned()]).expect("parse request");

    let feats = features_for(test, lane, &request).expect("features");

    assert!(
        !feats.contains("nb-detect"),
        "a lane whose packages do not declare the feature cannot be given it",
    );
}

#[test]
fn a_lane_without_the_virtual_clock_stays_without_it_when_asked_for_it() {
    let project = synthetic_project();
    let test = &project.test;
    let lane = &test.lanes["toolsmith"];
    let request = TestRequest::parse(&["--flash=on".to_owned()]).expect("parse request");

    let feats = features_for(test, lane, &request).expect("features");

    assert!(
        !feats.contains("virtual-time"),
        "a lane whose packages do not declare the feature cannot be given it",
    );
}

#[test]
fn no_block_bogus_mode_is_a_typed_error() {
    let error = TestRequest::parse(&["--no-block".to_owned(), "bogus".to_owned()])
        .expect_err("parse invalid no-block");

    assert!(error.to_string().contains("no-block"));
}

#[test]
fn no_block_space_form_parses_to_on() {
    let project = synthetic_project();
    let test = &project.test;
    let lane = &test.lanes[&test.default_lane];
    let request =
        TestRequest::parse(&["--no-block".to_owned(), "on".to_owned()]).expect("parse request");

    let feats = features_for(test, lane, &request).expect("features");
    assert!(feats.contains("nb-detect"));
}

#[test]
fn nextest_lane_command_shape() {
    let project = synthetic_project();
    let extra = vec!["--profile".to_owned(), "perf".to_owned()];

    let (features, cmd) = nextest_lane_command(
        &project,
        LaneToggles {
            flash: true,
            no_block: false,
        },
        "http",
        &extra,
    )
    .expect("nextest command");

    assert_eq!(
        features,
        vec!["base-feature".to_owned(), "virtual-time".to_owned()]
    );
    let args = args_of(&cmd);
    assert_eq!(cmd.get_program().to_string_lossy(), "cargo");
    assert!(args.windows(2).any(|w| w == ["nextest", "run"]));
    assert!(args.windows(2).any(|w| w == ["--profile", "perf"]));
    assert!(args.contains(&"--workspace".to_owned()));
    assert_eq!(args.last().map(String::as_str), Some("--locked"));
}

#[test]
fn nextest_run_preserves_prefix_with_global_args() {
    let mut project = synthetic_project();
    let prefix_args = vec![
        "nextest".to_owned(),
        "--color".to_owned(),
        "always".to_owned(),
        "run".to_owned(),
        "--workspace".to_owned(),
    ];
    let lane = project
        .test
        .lanes
        .get_mut("workspace")
        .expect("default lane");
    lane.prefix_args.clone_from(&prefix_args);

    let (_, cmd) = nextest_lane_command(
        &project,
        LaneToggles {
            flash: true,
            no_block: false,
        },
        "http",
        &[],
    )
    .expect("nextest run command");
    let args = args_of(&cmd);

    assert_eq!(&args[..prefix_args.len()], prefix_args.as_slice());
}

#[test]
fn package_scope_replaces_workspace_selection() {
    let mut project = synthetic_project();
    project
        .test
        .lanes
        .get_mut("workspace")
        .expect("workspace lane")
        .prefix_args
        .extend(["--exclude".to_owned(), "excluded-package".to_owned()]);
    let extra = vec!["-p".to_owned(), "one-package".to_owned()];

    let (_, cmd) = nextest_lane_command(
        &project,
        LaneToggles {
            flash: true,
            no_block: false,
        },
        "http",
        &extra,
    )
    .expect("nextest command");
    let args = args_of(&cmd);

    assert!(!args.contains(&"--workspace".to_owned()));
    assert!(!args.contains(&"--exclude".to_owned()));
    assert!(args.windows(2).any(|args| args == ["-p", "one-package"]));
}

#[test]
fn nextest_inventory_replaces_run_after_global_args() {
    let mut project = synthetic_project();
    let lane = project
        .test
        .lanes
        .get_mut("workspace")
        .expect("default lane");
    lane.prefix_args = vec![
        "nextest".to_owned(),
        "--color".to_owned(),
        "always".to_owned(),
        "run".to_owned(),
        "--workspace".to_owned(),
    ];

    let (_, cmd) = nextest_lane_command_for(
        &project,
        LaneToggles {
            flash: true,
            no_block: true,
        },
        "http",
        &["--message-format".to_owned(), "json".to_owned()],
        NextestAction::List,
    )
    .expect("nextest list command");
    let args = args_of(&cmd);

    let expected = ["nextest", "--color", "always", "list", "--workspace"].map(str::to_owned);
    assert_eq!(&args[..expected.len()], expected.as_slice());
    assert!(args.windows(2).any(|w| w == ["--message-format", "json"]));
    assert!(
        args.iter()
            .any(|arg| arg.split(',').any(|feature| feature == "nb-detect"))
    );
}

#[test]
fn loom_flag_selects_model_lane_and_composes_with_flash() {
    let project = synthetic_project();
    let request = TestRequest::parse(&["--loom=on".to_owned(), "--flash=on".to_owned()])
        .expect("parse request");

    let (name, lane) = select_lane(&project.test, &request).expect("select loom lane");
    assert_eq!(name, "loom");
    let features = features_for(&project.test, lane, &request).expect("loom features");
    assert_eq!(
        features,
        BTreeSet::from([
            "base-feature".to_owned(),
            "demo/loom".to_owned(),
            "virtual-time".to_owned(),
        ])
    );
}

#[test]
fn loom_flag_with_no_block_on_composes_features() {
    let project = synthetic_project();
    let request = TestRequest::parse(&["--loom=on".to_owned(), "--no-block=on".to_owned()])
        .expect("parse request");

    let (name, lane) = select_lane(&project.test, &request).expect("select loom lane");
    assert_eq!(name, "loom");
    let features = features_for(&project.test, lane, &request).expect("loom features");
    assert_eq!(
        features,
        BTreeSet::from([
            "base-feature".to_owned(),
            "demo/loom".to_owned(),
            "nb-detect".to_owned(),
        ])
    );
}

#[test]
fn loom_flag_rejects_an_explicit_non_model_lane() {
    let project = synthetic_project();
    let request = TestRequest::parse(&["--loom=on".to_owned(), "--lane=workspace".to_owned()])
        .expect("parse request");

    let error = select_lane(&project.test, &request).expect_err("lane conflict");
    assert!(
        error
            .to_string()
            .contains("conflicts with --lane=workspace")
    );
}

#[test]
fn a_lane_that_names_an_environment_runs_with_it() {
    let project = synthetic_project();
    let request = TestRequest::parse(&["--lane=browser".to_owned()]).expect("parse request");
    let (name, lane) = select_lane(&project.test, &request).expect("select browser lane");

    let cmd = lane_command(&project, name, lane, &request).expect("browser lane command");

    assert_eq!(
        envs_of(&cmd),
        vec![("DEMO_BROWSER".to_owned(), "firefox".to_owned())]
    );
}

#[test]
fn a_lane_that_names_its_own_profile_keeps_it_over_the_callers() {
    let mut project = synthetic_project();
    project.test.lanes.insert(
        "tooling".to_owned(),
        TestLaneConfig {
            program: "cargo".to_owned(),
            prefix_args: ["nextest", "run", "--profile", "support"]
                .map(str::to_owned)
                .to_vec(),
            ..TestLaneConfig::default()
        },
    );
    let request = TestRequest::parse(
        &[
            "--lane=tooling",
            "--profile",
            "ci",
            "--profile=ci",
            "--timings",
        ]
        .map(str::to_owned),
    )
    .expect("parse request");
    let (name, lane) = select_lane(&project.test, &request).expect("select tooling lane");

    let cmd = lane_command(&project, name, lane, &request).expect("tooling lane command");

    let args = args_of(&cmd);
    assert_eq!(
        args.iter()
            .filter(|arg| arg.starts_with("--profile"))
            .count(),
        1
    );
    assert!(args.windows(2).any(|pair| pair == ["--profile", "support"]));
    assert!(args.contains(&"--timings".to_owned()));
}

#[test]
fn a_lane_without_a_profile_takes_the_callers() {
    let project = synthetic_project();
    let request = TestRequest::parse(&["--lane=detector", "--profile", "ci"].map(str::to_owned))
        .expect("parse request");
    let (name, lane) = select_lane(&project.test, &request).expect("select detector lane");

    let cmd = lane_command(&project, name, lane, &request).expect("detector lane command");

    assert!(
        args_of(&cmd)
            .windows(2)
            .any(|pair| pair == ["--profile", "ci"])
    );
}

#[test]
fn a_lane_that_names_none_runs_with_none() {
    let project = synthetic_project();
    let request = TestRequest::parse(&[]).expect("parse request");
    let (name, lane) = select_lane(&project.test, &request).expect("select default lane");

    let cmd = lane_command(&project, name, lane, &request).expect("default lane command");

    assert!(envs_of(&cmd).is_empty());
}

/// A lane is clean only once its report agrees. The verdict lives past
/// the exit code because nextest exits zero on a retried pass, so a lane
/// read by status alone reported a defect that reproduced as a clean run.
///
/// The lane runs a program that exits without building anything while its
/// command line still names the runner profile it would have run under.
#[test]
fn a_green_status_is_not_the_whole_verdict_of_a_retrying_lane() {
    let temp = TempDir::new().expect("temp root");
    fs::create_dir_all(temp.path().join(".config")).expect("create .config");
    fs::write(
        temp.path().join(".config").join("nextest.toml"),
        "[profile.ci]\nretries = 1\n",
    )
    .expect("write nextest config");
    let mut project = synthetic_project();
    project.test.nextest_config = ".config/nextest.toml".to_owned();
    project.test.lanes.insert(
        "retrying".to_owned(),
        TestLaneConfig {
            program: "cargo".to_owned(),
            prefix_args: vec![
                "--version".to_owned(),
                "nextest".to_owned(),
                "run".to_owned(),
                "--profile".to_owned(),
                "ci".to_owned(),
            ],
            suffix_args: Vec::new(),
            default_features: Vec::new(),
            default_backend: None,
            default_flash: Some(false),
            default_no_block: Some(false),
            undeclared_toggles: Vec::new(),
            passthrough: String::new(),
            env: BTreeMap::new(),
            owns: Vec::new(),
        },
    );
    let request = TestRequest::parse(&[]).expect("parse request");
    let lane = &project.test.lanes["retrying"];

    let error = run_lane(&project, temp.path(), "retrying", lane, &request)
        .expect_err("a lane that ran a retrying profile is judged by its report")
        .to_string();

    assert!(error.contains("declares no JUnit report"), "{error}");
}

#[cfg(unix)]
fn failing_lane(prefix_args: &[&str]) -> TestLaneConfig {
    TestLaneConfig {
        program: "false".to_owned(),
        prefix_args: prefix_args.iter().map(|arg| (*arg).to_owned()).collect(),
        default_flash: Some(false),
        default_no_block: Some(false),
        ..TestLaneConfig::default()
    }
}

/// A red lane blames only what this run recorded: the report a previous
/// run left in a shared build directory must not name tests this one
/// never reached.
#[cfg(unix)]
#[test]
fn a_lane_that_fails_before_its_tests_does_not_inherit_the_previous_report() {
    let temp = TempDir::new().expect("temp root");
    fs::create_dir_all(temp.path().join(".config")).expect("create .config");
    fs::write(
        temp.path().join(".config").join("nextest.toml"),
        consts::RETRYING_PROFILE,
    )
    .expect("write nextest config");
    let store = temp.path().join("target").join("nextest").join("ci");
    fs::create_dir_all(&store).expect("create store");
    fs::write(store.join("junit.xml"), consts::FAILED_AND_RETRIED).expect("leave a report");
    let mut project = synthetic_project();
    project.test.nextest_config = ".config/nextest.toml".to_owned();
    let lane = failing_lane(&["nextest", "run", "--profile", "ci"]);
    let request = TestRequest::parse(&[]).expect("parse request");

    let error = run_lane(&project, temp.path(), "broken", &lane, &request)
        .expect_err("the lane exits non-zero")
        .to_string();

    assert!(error.contains("left no test report"), "{error}");
    assert!(!error.contains("stalled_target"), "{error}");
}

/// Removing the previous report only keeps the verdict honest: a report
/// the lane cannot remove must not stop the lane from running.
#[cfg(unix)]
#[test]
fn a_lane_whose_previous_report_cannot_be_removed_still_runs() {
    let temp = TempDir::new().expect("temp root");
    fs::create_dir_all(temp.path().join(".config")).expect("create .config");
    fs::write(
        temp.path().join(".config").join("nextest.toml"),
        consts::RETRYING_PROFILE,
    )
    .expect("write nextest config");
    let report = temp
        .path()
        .join("target")
        .join("nextest")
        .join("ci")
        .join("junit.xml");
    fs::create_dir_all(&report).expect("leave a report path no file removal clears");
    let mut project = synthetic_project();
    project.test.nextest_config = ".config/nextest.toml".to_owned();
    let lane = failing_lane(&["nextest", "run", "--profile", "ci"]);
    let request = TestRequest::parse(&[]).expect("parse request");

    let error = run_lane(&project, temp.path(), "broken", &lane, &request)
        .expect_err("the lane exits non-zero");

    assert_eq!(
        error
            .downcast_ref::<ChildFailure>()
            .map(ChildFailure::exit_code),
        Some(1),
        "{error:#}"
    );
}

/// The gate's last line names every red lane with its own reason, not
/// only that some failed.
#[cfg(unix)]
#[test]
fn every_red_touched_lane_keeps_its_own_error() {
    let mut project = synthetic_project();
    project
        .test
        .lanes
        .insert("first".to_owned(), failing_lane(&[]));
    project
        .test
        .lanes
        .insert("second".to_owned(), failing_lane(&[]));
    let request = TestRequest::parse(&[]).expect("parse request");

    let error = run_each(
        &project,
        Path::new("."),
        &request,
        &["first".to_owned(), "second".to_owned()],
    )
    .expect_err("both lanes fail");

    assert_eq!(
        error
            .downcast_ref::<ChildFailure>()
            .map(ChildFailure::exit_code),
        Some(1)
    );
    let error = error.to_string();
    assert!(
        error.contains("test lane `first` failed (exit code 1)"),
        "{error}"
    );
    assert!(
        error.contains("test lane `second` failed (exit code 1)"),
        "{error}"
    );
}

#[test]
fn a_tolerated_flake_without_an_owner_is_not_configuration() {
    let mut project = synthetic_project();
    project.test.known_flakes = vec![KnownFlake {
        test: "kithara_queue::delayed_target".to_owned(),
        issue: String::new(),
    }];

    let error = validate_config(&project.test)
        .expect_err("an entry nobody owns is a flake nobody removes")
        .to_string();

    assert!(error.contains("names no issue"), "{error}");
}

#[test]
fn a_configured_lane_carries_its_environment_to_the_runner() {
    let project = synthetic_project();

    let resolved = configured_lane(&project, "browser", "http", &[]).expect("configured lane");

    assert_eq!(resolved.env["DEMO_BROWSER"], "firefox");
    let (_, cmd) =
        nextest_configured_lane_command(&resolved, &[], NextestAction::Run).expect("command");
    assert_eq!(
        envs_of(&cmd),
        vec![("DEMO_BROWSER".to_owned(), "firefox".to_owned())]
    );
}

#[test]
fn a_platform_adapter_can_build_a_named_lane_inventory() {
    let project = synthetic_project();
    let command = nextest_command_for_lane(
        &project,
        "loom",
        &["-p".to_owned(), "demo-platform-tests".to_owned()],
        NextestAction::List,
    )
    .expect("named lane inventory");

    assert_eq!(
        args_of(&command),
        [
            "nextest",
            "list",
            "--features",
            "base-feature,demo/loom",
            "-p",
            "demo-platform-tests",
            "-E",
            "test(loom_model_)",
        ]
    );
}
