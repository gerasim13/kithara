use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::Command,
};

use anyhow::Result;
use tempfile::TempDir;

use super::{
    command::{execute, lane_command, run_each},
    request::TestRequest,
    selection::{lane_features, requested, select_lane, validate_config},
    *,
};
use crate::{
    common::project::{
        AuditClippyConfig, HealthConfig, KnownFlake, LintExcludeConfig, OrphansConfig, PerfConfig,
        ProjectConfig, ProjectIdentity, QualityConfig, StressConfig, TestCargoOptions,
        TestCargoRunner, TestCommandConfig, TestFlashConfig, TestLaneConfig, TestNetBackendConfig,
        TestNextestRunner, TestNoBlockConfig, TestRunner, WorkspaceScan,
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

/// One lane's command under `caller`, with the lane's own defaults.
fn command_of(
    project: &ProjectConfig,
    lane: &str,
    action: NextestAction,
    caller: &[&str],
) -> Result<Command> {
    let caller = caller
        .iter()
        .map(|arg| (*arg).to_owned())
        .collect::<Vec<_>>();
    resolve(&project.test, &requested(&project.test, lane, None)?)?.command(action, &caller)
}

/// A lane that sets every typed key, so a render shows the order of each.
fn typed_lane() -> TestLaneConfig {
    TestLaneConfig {
        default_flash: Some(false),
        cargo: TestCargoOptions {
            profile: Some("test-release".to_owned()),
            packages: vec!["demo".to_owned()],
            tests: vec!["suite".to_owned()],
            lib: true,
            ..TestCargoOptions::default()
        },
        runner: TestRunner::Nextest(TestNextestRunner {
            filter: Some("test(fast)".to_owned()),
            profile: Some("support".to_owned()),
            test_threads: Some(1),
            ignore_default_filter: true,
        }),
        ..TestLaneConfig::default()
    }
}

/// The features a lane resolves to for `request`.
pub(super) fn features_for(
    test: &TestCommandConfig,
    lane_name: &str,
    request: &TestRequest,
) -> Result<BTreeSet<String>> {
    let resolved = resolve(test, &requested(test, lane_name, Some(request))?)?;
    Ok(resolved.features.into_iter().collect())
}

fn synthetic_project() -> ProjectConfig {
    let packages = |names: &[&str]| TestCargoOptions {
        packages: names.iter().map(|name| (*name).to_owned()).collect(),
        ..TestCargoOptions::default()
    };
    let workspace = TestCargoOptions {
        workspace: true,
        ..TestCargoOptions::default()
    };
    let mut lanes = BTreeMap::new();
    lanes.insert(
        "workspace".to_owned(),
        TestLaneConfig {
            cargo: workspace.clone(),
            ..TestLaneConfig::default()
        },
    );
    lanes.insert(
        "loom".to_owned(),
        TestLaneConfig {
            default_flash: Some(false),
            cargo: workspace,
            runner: TestRunner::Nextest(TestNextestRunner {
                filter: Some("test(loom_model_)".to_owned()),
                ..TestNextestRunner::default()
            }),
            default_features: vec!["demo/loom".to_owned()],
            ..TestLaneConfig::default()
        },
    );
    lanes.insert(
        "toolsmith".to_owned(),
        TestLaneConfig {
            cargo: packages(&["demo-tools"]),
            undeclared_toggles: vec![
                consts::FLASH_TOGGLE.to_owned(),
                consts::NO_BLOCK_TOGGLE.to_owned(),
            ],
            ..TestLaneConfig::default()
        },
    );
    lanes.insert(
        "detector".to_owned(),
        TestLaneConfig {
            default_no_block: Some(true),
            cargo: packages(&["demo-detector"]),
            ..TestLaneConfig::default()
        },
    );
    lanes.insert(
        "browser".to_owned(),
        TestLaneConfig {
            env: BTreeMap::from([("DEMO_BROWSER".to_owned(), "firefox".to_owned())]),
            default_flash: Some(false),
            cargo: TestCargoOptions {
                tests: vec!["web".to_owned()],
                ..packages(&["demo-web"])
            },
            runner: TestRunner::Cargo(TestCargoRunner {
                name_filters: vec!["selenium".to_owned()],
                no_capture: true,
                ..TestCargoRunner::default()
            }),
            ..TestLaneConfig::default()
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
    let features = features_for(&project.test, "workspace", &default).unwrap();
    let explicit =
        TestRequest::parse(&["--flash=off".into(), "--net-backend=native".into()]).unwrap();
    let requested = features_for(&project.test, "workspace", &explicit).unwrap();
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
    let request = TestRequest::parse(&[]).expect("parse request");

    let feats = features_for(test, &test.default_lane, &request).expect("features");
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

    let lane_backend = lane_command(
        &project.test,
        &project.test.default_lane,
        &TestRequest::parse(&[]).expect("parse request"),
    )
    .expect("default lane command");
    assert!(
        args_of(&lane_backend)
            .iter()
            .any(|arg| arg.contains("demo/native-net"))
    );

    let cli_backend = lane_command(
        &project.test,
        &project.test.default_lane,
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
    let request = TestRequest::parse(&[
        "--flash=on".to_owned(),
        "--no-block=on".to_owned(),
        "--net-backend=native".to_owned(),
    ])
    .expect("parse request");

    let feats = features_for(test, &test.default_lane, &request).expect("features");
    assert!(feats.contains("base-feature"));
    assert!(feats.contains("virtual-time"));
    assert!(feats.contains("demo/native-net"));
    assert!(feats.contains("nb-detect"));
}

#[test]
fn no_block_off_keeps_no_block_features_out() {
    let project = synthetic_project();
    let test = &project.test;
    let request = TestRequest::parse(&[
        "--flash=on".to_owned(),
        "--no-block=off".to_owned(),
        "--net-backend=native".to_owned(),
    ])
    .expect("parse request");

    let feats = features_for(test, &test.default_lane, &request).expect("features");
    assert!(!feats.contains("nb-detect"));
    assert!(feats.contains("virtual-time"));
}

#[test]
fn a_lane_that_asks_for_the_detector_gets_it_without_a_flag() {
    let project = synthetic_project();
    let test = &project.test;
    let request = TestRequest::parse(&[]).expect("parse request");

    let feats = features_for(test, "detector", &request).expect("features");

    assert!(feats.contains("nb-detect"));
}

#[test]
fn an_explicit_off_overrides_the_lane_detector_default() {
    let project = synthetic_project();
    let test = &project.test;
    let request = TestRequest::parse(&["--no-block=off".to_owned()]).expect("parse request");

    let feats = features_for(test, "detector", &request).expect("features");

    assert!(!feats.contains("nb-detect"));
}

#[test]
fn a_lane_without_the_detector_stays_without_it_when_the_gate_asks_for_it() {
    let project = synthetic_project();
    let test = &project.test;
    let request = TestRequest::parse(&["--no-block=on".to_owned()]).expect("parse request");

    let feats = features_for(test, "toolsmith", &request).expect("features");

    assert!(
        !feats.contains("nb-detect"),
        "a lane whose packages do not declare the feature cannot be given it",
    );
}

#[test]
fn a_lane_without_the_virtual_clock_stays_without_it_when_asked_for_it() {
    let project = synthetic_project();
    let test = &project.test;
    let request = TestRequest::parse(&["--flash=on".to_owned()]).expect("parse request");

    let feats = features_for(test, "toolsmith", &request).expect("features");

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
    let request =
        TestRequest::parse(&["--no-block".to_owned(), "on".to_owned()]).expect("parse request");

    let feats = features_for(test, &test.default_lane, &request).expect("features");
    assert!(feats.contains("nb-detect"));
}

#[test]
fn the_default_lane_renders_its_selection_features_and_the_callers_profile() {
    let project = synthetic_project();

    let command = command_of(
        &project,
        "workspace",
        NextestAction::Run,
        &["--profile", "perf"],
    )
    .expect("command");

    assert_eq!(command.get_program().to_string_lossy(), "cargo");
    assert_eq!(
        args_of(&command),
        [
            "nextest",
            "run",
            "--workspace",
            "--features",
            "base-feature,virtual-time",
            "--profile",
            "perf",
        ]
    );
}

#[test]
fn package_scope_replaces_workspace_selection() {
    let mut project = synthetic_project();
    project
        .test
        .lanes
        .get_mut("workspace")
        .expect("workspace lane")
        .cargo
        .exclude
        .push("excluded-package".to_owned());

    let args = args_of(
        &command_of(
            &project,
            "workspace",
            NextestAction::Run,
            &["-p", "one-package"],
        )
        .expect("command"),
    );

    assert!(!args.contains(&"--workspace".to_owned()));
    assert!(!args.contains(&"--exclude".to_owned()));
    assert!(args.windows(2).any(|args| args == ["-p", "one-package"]));
}

#[test]
fn the_inventory_lists_the_same_selection_the_run_builds() {
    let project = synthetic_project();
    let resolved = resolve(
        &project.test,
        &LaneChoice {
            features: &[],
            backend: "http",
            lane: "workspace",
            toggles: LaneToggles {
                flash: true,
                no_block: true,
            },
        },
    )
    .expect("resolve");

    let command = resolved
        .command(
            NextestAction::List,
            &["--message-format".to_owned(), "json".to_owned()],
        )
        .expect("list command");

    assert_eq!(
        args_of(&command),
        [
            "nextest",
            "list",
            "--workspace",
            "--features",
            "base-feature,nb-detect,virtual-time",
            "--message-format",
            "json",
        ]
    );
}

#[test]
fn loom_flag_selects_model_lane_and_composes_with_flash() {
    let project = synthetic_project();
    let request = TestRequest::parse(&["--loom=on".to_owned(), "--flash=on".to_owned()])
        .expect("parse request");

    let name = select_lane(&project.test, &request).expect("select loom lane");
    assert_eq!(name, "loom");
    let features = features_for(&project.test, name, &request).expect("loom features");
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

    let name = select_lane(&project.test, &request).expect("select loom lane");
    assert_eq!(name, "loom");
    let features = features_for(&project.test, name, &request).expect("loom features");
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
    let name = select_lane(&project.test, &request).expect("select browser lane");

    let cmd = lane_command(&project.test, name, &request).expect("browser lane command");

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
            cargo: TestCargoOptions {
                packages: vec!["demo-tools".to_owned()],
                ..TestCargoOptions::default()
            },
            runner: TestRunner::Nextest(TestNextestRunner {
                profile: Some("support".to_owned()),
                ..TestNextestRunner::default()
            }),
            ..TestLaneConfig::default()
        },
    );
    let request = TestRequest::parse(
        &[
            "--lane=tooling",
            "--profile",
            "ci",
            "--profile=ci",
            "-P",
            "ci",
            "--timings",
        ]
        .map(str::to_owned),
    )
    .expect("parse request");
    let name = select_lane(&project.test, &request).expect("select tooling lane");

    let cmd = lane_command(&project.test, name, &request).expect("tooling lane command");

    let args = args_of(&cmd);
    assert_eq!(
        args.iter()
            .filter(|arg| arg.starts_with("--profile"))
            .count(),
        1
    );
    assert!(args.windows(2).any(|pair| pair == ["--profile", "support"]));
    assert!(!args.contains(&"-P".to_owned()), "{args:?}");
    assert!(args.contains(&"--timings".to_owned()));
}

#[test]
fn a_lane_without_a_profile_takes_the_callers() {
    let project = synthetic_project();
    let request = TestRequest::parse(&["--lane=detector", "--profile", "ci"].map(str::to_owned))
        .expect("parse request");
    let name = select_lane(&project.test, &request).expect("select detector lane");

    let cmd = lane_command(&project.test, name, &request).expect("detector lane command");

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
    let name = select_lane(&project.test, &request).expect("select default lane");

    let cmd = lane_command(&project.test, name, &request).expect("default lane command");

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
    let mut command = Command::new("cargo");
    command.args(["--version", "nextest", "run", "--profile", "ci"]);

    let error = execute(&project.test, temp.path(), "retrying", &mut command)
        .expect_err("a lane that ran a retrying profile is judged by its report")
        .to_string();

    assert!(error.contains("declares no JUnit report"), "{error}");
}

#[cfg(unix)]
fn failing(args: &[&str]) -> Command {
    let mut command = Command::new("false");
    command.args(args);
    command
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

    let error = execute(
        &project.test,
        temp.path(),
        "broken",
        &mut failing(&["nextest", "run", "--profile", "ci"]),
    )
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

    let error = execute(
        &project.test,
        temp.path(),
        "broken",
        &mut failing(&["nextest", "run", "--profile", "ci"]),
    )
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
    let project = synthetic_project();

    let error = run_each(&["first".to_owned(), "second".to_owned()], |lane| {
        execute(&project.test, Path::new("."), lane, &mut failing(&[]))
    })
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
fn a_resolved_lane_carries_its_environment_to_the_runner() {
    let project = synthetic_project();

    let resolved = resolve(
        &project.test,
        &requested(&project.test, "browser", None).expect("lane"),
    )
    .expect("resolve");

    assert_eq!(resolved.env["DEMO_BROWSER"], "firefox");
    let command = resolved.command(NextestAction::Run, &[]).expect("command");
    assert_eq!(
        envs_of(&command),
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
            "-E",
            "test(loom_model_)",
            "-p",
            "demo-platform-tests",
        ]
    );
}

#[test]
fn a_nextest_lane_renders_its_typed_options_in_one_order() {
    let mut project = synthetic_project();
    project.test.lanes.insert("typed".to_owned(), typed_lane());

    let command =
        command_of(&project, "typed", NextestAction::Run, &["--timings"]).expect("command");

    assert_eq!(
        args_of(&command),
        [
            "nextest",
            "run",
            "--profile",
            "support",
            "-p",
            "demo",
            "--cargo-profile",
            "test-release",
            "--lib",
            "--test",
            "suite",
            "--features",
            "base-feature",
            "--test-threads",
            "1",
            "--ignore-default-filter",
            "-E",
            "test(fast)",
            "--timings",
        ]
    );
}

/// `nextest list` refuses `--test-threads`; a thread count only shapes a
/// run.
#[test]
fn listing_a_lane_leaves_its_thread_count_to_the_run() {
    let mut project = synthetic_project();
    project.test.lanes.insert("typed".to_owned(), typed_lane());

    let args = args_of(&command_of(&project, "typed", NextestAction::List, &[]).expect("list"));

    assert_eq!(
        args.get(..2),
        Some(["nextest", "list"].map(str::to_owned).as_slice())
    );
    assert!(!args.contains(&"--test-threads".to_owned()), "{args:?}");
}

#[test]
fn a_cargo_lane_hands_its_name_filters_to_the_test_binary() {
    let project = synthetic_project();

    let command = command_of(
        &project,
        "browser",
        NextestAction::Run,
        &["--timings", "--", "--ignored"],
    )
    .expect("command");

    assert_eq!(
        args_of(&command),
        [
            "test",
            "-p",
            "demo-web",
            "--test",
            "web",
            "--features",
            "base-feature",
            "--timings",
            "--",
            "selenium",
            "--nocapture",
            "--ignored",
        ]
    );
}

#[test]
fn a_doc_lane_runs_the_doctests_of_its_selection() {
    let mut project = synthetic_project();
    project.test.lanes.insert(
        "doc".to_owned(),
        TestLaneConfig {
            default_flash: Some(false),
            cargo: TestCargoOptions {
                profile: Some("test-release".to_owned()),
                exclude: vec!["demo-fuzz".to_owned()],
                workspace: true,
                ..TestCargoOptions::default()
            },
            runner: TestRunner::Cargo(TestCargoRunner {
                doc: true,
                ..TestCargoRunner::default()
            }),
            ..TestLaneConfig::default()
        },
    );

    let command =
        command_of(&project, "doc", NextestAction::Run, &["--profile", "ci"]).expect("command");

    assert_eq!(
        args_of(&command),
        [
            "test",
            "--doc",
            "--workspace",
            "--exclude",
            "demo-fuzz",
            "--profile",
            "test-release",
            "--features",
            "base-feature",
        ]
    );
}

#[test]
fn a_cargo_lane_has_no_inventory_to_list() {
    let project = synthetic_project();

    let error = command_of(&project, "browser", NextestAction::List, &[])
        .expect_err("cargo test lists nothing")
        .to_string();

    assert!(error.contains("has no inventory to list"), "{error}");
}

#[test]
fn a_lane_selects_exactly_one_of_the_workspace_and_its_packages() {
    for cargo in [
        TestCargoOptions::default(),
        TestCargoOptions {
            packages: vec!["demo".to_owned()],
            workspace: true,
            ..TestCargoOptions::default()
        },
    ] {
        let mut project = synthetic_project();
        project.test.lanes.insert(
            "odd".to_owned(),
            TestLaneConfig {
                cargo,
                ..TestLaneConfig::default()
            },
        );

        let error = validate_config(&project.test)
            .expect_err("a lane selects one way")
            .to_string();

        assert!(
            error.contains("test.lanes.odd.cargo needs exactly one"),
            "{error}"
        );
    }
}

#[test]
fn an_exclusion_needs_a_workspace_lane() {
    let mut project = synthetic_project();
    project.test.lanes.insert(
        "odd".to_owned(),
        TestLaneConfig {
            cargo: TestCargoOptions {
                exclude: vec!["demo-fuzz".to_owned()],
                packages: vec!["demo".to_owned()],
                ..TestCargoOptions::default()
            },
            ..TestLaneConfig::default()
        },
    );

    let error = validate_config(&project.test)
        .expect_err("exclude narrows only a workspace")
        .to_string();

    assert!(
        error.contains("test.lanes.odd.cargo.exclude needs"),
        "{error}"
    );
}

/// A caller's package selection replaces any lane's workspace
/// selection, so `-p` narrows the doc and loom lanes as it narrows the
/// default one.
#[test]
fn a_caller_package_narrows_every_workspace_lane() {
    let project = synthetic_project();

    let args = args_of(
        &command_of(
            &project,
            "loom",
            NextestAction::Run,
            &["--package=demo-core"],
        )
        .expect("command"),
    );

    assert!(!args.contains(&"--workspace".to_owned()), "{args:?}");
    assert!(args.contains(&"--package=demo-core".to_owned()), "{args:?}");
}

/// The one difference between the two runners' cargo arguments is the
/// spelling of the Cargo profile.
#[test]
fn the_runners_translate_only_the_profile_flag() {
    let mut project = synthetic_project();
    project.test.lanes.insert("typed".to_owned(), typed_lane());
    let resolved = resolve(
        &project.test,
        &requested(&project.test, "typed", None).expect("lane"),
    )
    .expect("resolve");

    let for_nextest = resolved.cargo_args("--cargo-profile", false);
    let for_cargo = resolved.cargo_args("--profile", false);

    assert_eq!(
        for_nextest
            .iter()
            .map(|arg| if arg == "--cargo-profile" {
                "--profile"
            } else {
                arg.as_str()
            })
            .collect::<Vec<_>>(),
        for_cargo
    );
}

/// nextest unions repeated `-E`, so a caller filterset would widen a
/// filtered lane; the lane's filter and the caller's are intersected into
/// one expression instead, whatever spelling the caller used.
#[test]
fn a_caller_filterset_narrows_a_filtered_lane() {
    let project = synthetic_project();

    let args = args_of(
        &command_of(
            &project,
            "loom",
            NextestAction::Run,
            &[
                "-E",
                "test(seek)",
                "--filterset",
                "test(a)",
                "--filter-expr=test(b)",
                "-Etest(c)",
                "--timings",
            ],
        )
        .expect("command"),
    );

    assert_eq!(
        args.iter().filter(|arg| *arg == "-E").count(),
        1,
        "{args:?}"
    );
    assert!(
        args.windows(2).any(|pair| pair
            == [
                "-E",
                "(test(loom_model_)) & ((test(seek)) | (test(a)) | (test(b)) | (test(c)))"
            ]),
        "{args:?}"
    );
    assert!(
        !args.iter().any(|arg| arg.starts_with("--filter")),
        "{args:?}"
    );
    assert_eq!(args.last().map(String::as_str), Some("--timings"));
}

#[test]
fn a_caller_filterset_passes_through_an_unfiltered_lane() {
    let project = synthetic_project();

    let args = args_of(
        &command_of(
            &project,
            "workspace",
            NextestAction::Run,
            &["-E", "test(seek)"],
        )
        .expect("command"),
    );

    assert!(
        args.ends_with(&["-E".to_owned(), "test(seek)".to_owned()]),
        "{args:?}"
    );
}
