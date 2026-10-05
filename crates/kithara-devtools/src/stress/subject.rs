use super::*;
use crate::{
    common::project::{TestCargoOptions, TestRunner},
    test::ResolvedLane,
};

const REPORT: &str = "command-junit.xml";
const STALE_REPORT: &str = "controller report from an earlier run";

fn subject_fixture(root: &Path) -> (Ctx, RunArgs) {
    let controller = root.join("controller");
    let subject = root.join("subject");
    for (checkout, identity) in [(&controller, "controller"), (&subject, "subject")] {
        fs::create_dir_all(checkout).expect("create checkout");
        fs::write(checkout.join("identity"), identity).expect("write checkout identity");
    }
    fs::write(controller.join("nextest.toml"), "").expect("write nextest config");
    fs::write(controller.join(REPORT), STALE_REPORT).expect("write stale controller report");
    let executable = std::env::current_exe().expect("current test executable");
    let mode = StressModeConfig {
        command: std::iter::once(executable.to_string_lossy().into_owned())
            .chain(crate::common::child_test_args(
                module_path!(),
                "record_subject_execution",
            ))
            .collect(),
        attempt_junit: Some(REPORT.to_owned()),
        ..StressModeConfig::default()
    };
    let project = ProjectConfig {
        stress: StressConfig {
            modes: BTreeMap::from([("command".to_owned(), mode)]),
            default_modes: vec!["command".to_owned()],
            default_filter: "all()".to_owned(),
            build_dir: "target-stress".to_owned(),
            nextest_config: "nextest.toml".to_owned(),
            nextest_profile: "stress".to_owned(),
            test_threads: "1".to_owned(),
            default_count: 1,
            max_count: 1,
            max_test_threads: 1,
            workflow_job_timeout_minutes: 1,
            artifacts: StressArtifactConfig {
                log: "lane.log".to_owned(),
                manifest: "manifest.json".to_owned(),
                pressure: "pressure.jsonl".to_owned(),
                attempts: "attempts.json".to_owned(),
                subject_junit: "nextest-junit.xml".to_owned(),
                ..StressArtifactConfig::default()
            },
            ..StressConfig::default()
        },
        ..ProjectConfig::default()
    };
    let args = RunArgs {
        count: None,
        expected_controller_sha: None,
        expected_subject_sha: None,
        filter: None,
        output: Some(root.join("raw")),
        subject_root: subject,
        modes: Vec::new(),
        lanes: Vec::new(),
    };
    (Ctx::new(controller, project), args)
}

fn assert_subject_report(paths: &Paths, ctx: &Ctx, args: &RunArgs) {
    let report = fs::read_to_string(paths.attempt_junit.join("attempt-0.xml"))
        .expect("archive the subject command report");
    let observed: Vec<String> = serde_json::from_str(&report).expect("execution record");
    assert_eq!(Path::new(&observed[0]), args.subject_root);
    assert_eq!(
        Path::new(&observed[1]),
        args.subject_root.join("target-stress")
    );
    assert_eq!(observed[2], "subject");
    assert_eq!(
        fs::read_to_string(ctx.root.join(REPORT)).expect("keep controller report"),
        STALE_REPORT
    );
}

#[test]
fn command_modes_execute_and_archive_the_subject_checkout() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (ctx, args) = subject_fixture(temp.path());
    assert_ne!(ctx.root, args.subject_root);
    let config = &ctx.config.stress;
    let mode = config.mode("command").expect("command mode");
    let paths = Paths::new(temp.path().join("raw"), &config.artifacts);
    let environment = RunEnvironment::new(
        &paths.raw,
        &args.subject_root.join(&config.build_dir),
        config,
        mode,
    )
    .expect("subject environment");
    fs::create_dir_all(&paths.raw).expect("create raw directory");

    let codes = run_command_lane(&args.subject_root, mode, &paths, 1, &environment)
        .expect("execute subject command mode");

    assert_eq!(codes, [0]);
    assert_subject_report(&paths, &ctx, &args);
}

#[test]
fn command_modes_reject_filtersets_before_creating_evidence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (ctx, mut args) = subject_fixture(temp.path());
    args.filter = Some("test(selected)".to_owned());

    let error = execute_run(&args, &ctx).expect_err("command modes cannot narrow by filterset");

    assert!(error.to_string().contains("--filter"), "{error:#}");
    assert!(error.to_string().contains("command"), "{error:#}");
    assert!(!args.output.expect("raw path").exists());
    assert!(!ctx.root.join("target-stress").exists());
    assert!(!args.subject_root.join("target-stress").exists());
    assert!(!args.subject_root.join(REPORT).exists());
}

#[test]
fn command_reports_reject_filtersets_before_creating_output() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (ctx, _) = subject_fixture(temp.path());
    let output = temp.path().join("report-output/report.md");
    let args = ReportArgs {
        execute_result: ExecuteResult::Success,
        count: None,
        filter: Some("test(selected)".to_owned()),
        output: Some(output.clone()),
        raw: temp.path().join("raw"),
        expected_controller_sha: "a".repeat(40),
        expected_subject_sha: "b".repeat(40),
        modes: Vec::new(),
        lanes: Vec::new(),
    };

    let error = run_report(&args, &ctx).expect_err("command reports cannot claim a filterset");

    assert!(error.to_string().contains("--filter"), "{error:#}");
    assert!(error.to_string().contains("command"), "{error:#}");
    assert!(!output.parent().expect("report parent").exists());
}

#[test]
fn command_modes_reject_configured_filtersets_without_side_effects() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (mut ctx, args) = subject_fixture(temp.path());
    ctx.config.stress.default_filter = "test(selected)".to_owned();

    let error = execute_run(&args, &ctx).expect_err("configured command filterset");

    assert!(
        error.to_string().contains("stress.default_filter"),
        "{error:#}"
    );
    assert!(!args.output.as_ref().expect("raw path").exists());
    assert!(!args.subject_root.join("target-stress").exists());
    assert_eq!(
        fs::read_to_string(ctx.root.join(REPORT)).expect("keep controller report"),
        STALE_REPORT
    );
    let output = temp.path().join("report-output/report.md");
    let report = ReportArgs {
        execute_result: ExecuteResult::Success,
        count: None,
        filter: None,
        output: Some(output.clone()),
        raw: temp.path().join("raw"),
        expected_controller_sha: "a".repeat(40),
        expected_subject_sha: "b".repeat(40),
        modes: Vec::new(),
        lanes: Vec::new(),
    };

    let error = run_report(&report, &ctx).expect_err("configured command report filterset");

    assert!(
        error.to_string().contains("stress.default_filter"),
        "{error:#}"
    );
    assert!(!output.parent().expect("report parent").exists());
}

#[test]
fn filtersets_remain_supported_for_lane_modes() {
    let mode = StressModeConfig::default();
    let unit = Unit {
        mode_name: "lane",
        mode: &mode,
        lane: Some("tests"),
        runner: StressRunner::Lane(Box::new(ResolvedLane {
            env: BTreeMap::new(),
            backend: "local".to_owned(),
            lane: "tests".to_owned(),
            cargo: TestCargoOptions::default(),
            runner: TestRunner::default(),
            features: Vec::new(),
        })),
    };

    validate_filter(Some("test(selected)"), "all()", std::slice::from_ref(&unit))
        .expect("lane modes honor explicit filtersets");
    validate_filter(None, "test(selected)", &[unit])
        .expect("lane modes honor configured filtersets");
    let command = StressModeConfig {
        command: vec!["just".to_owned(), "test".to_owned(), "rtsan".to_owned()],
        ..StressModeConfig::default()
    };
    let unit = Unit {
        mode_name: "command",
        mode: &command,
        lane: None,
        runner: StressRunner::Command(command.command.clone()),
    };

    validate_filter(None, "all()", &[unit]).expect("unfiltered command modes remain supported");
}

#[cfg(target_os = "linux")]
#[test]
fn command_mode_lifecycle_records_the_subject_build_and_revision() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (ctx, args) = subject_fixture(temp.path());
    for root in [&ctx.root, &args.subject_root] {
        for arguments in [
            vec!["init"],
            vec!["add", "identity"],
            vec![
                "-c",
                "user.name=Stress Test",
                "-c",
                "user.email=stress@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "checkout identity",
            ],
        ] {
            let output = Command::new("git")
                .args(arguments)
                .current_dir(root)
                .output()
                .expect("prepare checkout revision");
            assert!(output.status.success(), "{output:?}");
        }
    }
    let config = &ctx.config.stress;
    let paths = Paths::new(
        args.output.clone().expect("raw path").join("command"),
        &config.artifacts,
    );

    execute_run(&args, &ctx).expect("run command mode lifecycle");

    assert_subject_report(&paths, &ctx, &args);
    let manifest = Manifest::read(&paths.manifest).expect("read command manifest");
    assert_eq!(
        Path::new(&manifest.build.target_dir),
        args.subject_root.join(&config.build_dir)
    );
    assert_eq!(
        manifest.subject.sha,
        revision(&args.subject_root, None, "subject").expect("subject revision")
    );
    assert_ne!(manifest.subject.sha, manifest.controller.sha);
}

#[test]
#[ignore = "subprocess entrypoint"]
fn record_subject_execution() {
    let cwd = std::env::current_dir().expect("child cwd");
    let target = std::env::var(consts::TARGET_DIR_ENV).expect("child build directory");
    let identity = fs::read_to_string(cwd.join("identity")).expect("child checkout identity");
    let record = serde_json::to_string(&[cwd.to_string_lossy().into_owned(), target, identity])
        .expect("serialize execution record");
    fs::write(cwd.join(REPORT), record).expect("write child command report");
}
