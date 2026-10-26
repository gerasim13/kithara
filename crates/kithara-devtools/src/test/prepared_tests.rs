#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{collections::BTreeMap, fs, path::Path, process::Command};

use tempfile::TempDir;

#[cfg(unix)]
use super::inventory_tests::{children, fixture};
use super::{
    NextestAction, NextestPlan, PreparedNextest, nextest_plans, request::TestRequest,
    tests::synthetic_project,
};
#[cfg(unix)]
use crate::common::project::{ProjectConfig, TestRunner};

fn args(command: &Command) -> Vec<String> {
    command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn archives_keep_build_inputs_without_test_selection_or_run_policy() {
    for flash in ["--flash=on", "--flash=off"] {
        let mut project = synthetic_project();
        let lane = project.test.lanes.get_mut("loom").unwrap();
        lane.cargo.profile = Some("test-release".to_owned());
        let extra = [
            "--lane=loom",
            flash,
            "--profile",
            "ci",
            "--retries=0",
            "--test-threads",
            "4",
            "--narrow=test(one)",
            "--timings",
            "--no-default-features",
            "-p",
            "demo",
            "--",
            "one",
            "--skip",
            "two",
        ]
        .map(str::to_owned);
        let plans = nextest_plans(Path::new("."), &project, &extra, "main").unwrap();
        assert_eq!(plans.len(), 1);
        let command = plans[0]
            .archive_command(Path::new("tests.tar.zst"))
            .unwrap();
        let mut expected = vec![
            "nextest",
            "archive",
            "--cargo-profile",
            "test-release",
            "--features",
        ];
        expected.push(if flash == "--flash=on" {
            "base-feature,demo/loom,virtual-time"
        } else {
            "base-feature,demo/loom"
        });
        expected.extend([
            "--profile",
            "ci",
            "--timings",
            "--no-default-features",
            "-p",
            "demo",
            "--archive-file",
            "tests.tar.zst",
        ]);
        assert_eq!(args(&command), expected);
        let roundtrip: NextestPlan =
            serde_json::from_slice(&serde_json::to_vec(&plans[0]).unwrap()).unwrap();
        assert_eq!(plans[0], roundtrip);
    }
}

#[test]
fn ordinary_commands_refuse_archive_passthrough_and_keep_runner_arguments() {
    let project = synthetic_project();
    for flag in [
        "--archive-file",
        "--archive-format",
        "--extract-to",
        "--workspace-remap",
        "--binaries-metadata",
        "--cargo-metadata",
        "--target-dir-remap",
        "--build-dir-remap",
    ] {
        let extra = [format!("{flag}=other")];
        assert!(TestRequest::parse(&extra).is_err());
        assert!(
            super::nextest_command_for_lane(&project, "workspace", &extra, NextestAction::Run)
                .is_err()
        );
    }
    let extra = ["--profile", "ci", "--retries=0"].map(str::to_owned);
    let command =
        super::nextest_command_for_lane(&project, "workspace", &extra, NextestAction::Run).unwrap();
    assert_eq!(
        args(&command),
        [
            "nextest",
            "run",
            "--workspace",
            "--features",
            "base-feature,virtual-time",
            "--profile",
            "ci",
            "--retries=0"
        ]
    );
    for extra in [
        vec!["--unknown-option", "value"],
        vec!["--no-run"],
        vec!["--run-ignored=all"],
        vec!["--retries"],
    ] {
        assert!(
            nextest_plans(
                Path::new("."),
                &project,
                &extra.into_iter().map(str::to_owned).collect::<Vec<_>>(),
                "main"
            )
            .is_err()
        );
    }
}

#[test]
fn public_touched_plans_narrow_packages_and_serialize_empty_branches() {
    for changed in ["tools/lib.rs", "web/lib.rs"] {
        let directory = TempDir::new().unwrap();
        let root = directory.path().join("work");
        fs::create_dir_all(root.join("tools")).unwrap();
        fs::create_dir_all(root.join("web")).unwrap();
        let hooks = directory.path().join("empty-hooks");
        fs::create_dir(&hooks).unwrap();
        let hooks = format!("core.hooksPath={}", hooks.display());
        let git = |dir: &Path, args: &[&str]| {
            let status = Command::new("git")
                .current_dir(dir)
                .args([
                    "-c",
                    "user.name=planner",
                    "-c",
                    "user.email=planner@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    &hooks,
                ])
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "protocol.file.allow", "always"]);
        fs::write(root.join("base.txt"), "base").unwrap();
        git(&root, &["add", "base.txt"]);
        git(&root, &["commit", "-q", "-m", "base"]);
        git(
            directory.path(),
            &["clone", "-q", "--bare", "work", "origin.git"],
        );
        let origin = directory.path().join("origin.git");
        git(
            &root,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(&root, &["checkout", "-q", "-b", "feature"]);
        fs::write(root.join(changed), "changed").unwrap();
        git(&root, &["add", changed]);
        git(&root, &["commit", "-q", "-m", "change"]);
        let mut project = synthetic_project();
        project
            .test
            .lanes
            .get_mut("workspace")
            .unwrap()
            .cargo
            .exclude = vec!["demo-web".to_owned()];
        project.test.lanes.get_mut("toolsmith").unwrap().owns = vec!["tools/".to_owned()];
        project.test.lanes.get_mut("browser").unwrap().owns = vec!["web/".to_owned()];
        let plans = nextest_plans(
            &root,
            &project,
            &[
                "--touched",
                "--lane=workspace",
                "--flash=off",
                "--selection-output=selection.json",
            ]
            .map(str::to_owned),
            "main",
        )
        .unwrap();
        let selected = fs::read_to_string(root.join("selection.json")).unwrap();
        if changed.starts_with("web/") {
            assert_eq!(serde_json::to_string(&plans).unwrap(), "[]");
            assert_eq!(selected, "[]");
        } else {
            assert_eq!(plans.len(), 1);
            assert_eq!(plans[0].lane(), "workspace");
            assert_eq!(selected, "[\"workspace\"]");
            let recorded = serde_json::to_value(&plans).unwrap();
            assert_eq!(
                recorded[0]["lane"]["runner"]["nextest"]["filter"],
                "package(demo-tools)"
            );
            let build = plans[0]
                .archive_command(Path::new("tests.tar.zst"))
                .unwrap();
            assert_eq!(
                args(&build),
                [
                    "nextest",
                    "archive",
                    "--workspace",
                    "--exclude",
                    "demo-web",
                    "--features",
                    "base-feature",
                    "--archive-file",
                    "tests.tar.zst"
                ]
            );
        }
    }
}

#[cfg(unix)]
fn environment() -> BTreeMap<String, String> {
    BTreeMap::from([("TEST_RUNTIME_READY".to_owned(), "1".to_owned())])
}

#[cfg(unix)]
fn prepared_fixture() -> (TempDir, ProjectConfig, NextestPlan) {
    let (temp, test, _) = fixture(&[("demo::tests", "one")], b"demo::tests\tone\n");
    let root = temp.path();
    fs::write(root.join("prepared.tar.zst"), "archive\n").unwrap();
    fs::write(root.join("model.bin"), "model").unwrap();
    let forbidden = root.join("cargo");
    fs::write(
        &forbidden,
        "#!/bin/sh\nprintf 'compile\\n' >> \"$TEST_CHILDREN\"\nexit 94\n",
    )
    .unwrap();
    fs::set_permissions(&forbidden, fs::Permissions::from_mode(0o755)).unwrap();
    for name in ["rustc", "just"] {
        fs::copy(&forbidden, root.join(name)).unwrap();
    }
    let mut project = synthetic_project();
    project.test = test;
    let lane = project.test.lanes.get_mut("workspace").unwrap();
    lane.cargo.profile = Some("test-release".to_owned());
    let TestRunner::Nextest(nextest) = &mut lane.runner else {
        panic!("nextest fixture")
    };
    nextest.ignore_default_filter = true;
    nextest.test_threads = Some(2);
    lane.env.extend([
        ("CARGO".to_owned(), forbidden.display().to_string()),
        ("TEST_PREPARED".to_owned(), "1".to_owned()),
        (
            "TEST_ARCHIVE".to_owned(),
            root.join("prepared.tar.zst").display().to_string(),
        ),
    ]);
    let mut arguments = [
        "--profile=default",
        "--retries=0",
        "--narrow=test(one)",
        "--timings",
    ]
    .map(str::to_owned)
    .to_vec();
    arguments.push(format!(
        "--expected-tests={}",
        root.join("expected.tsv").display()
    ));
    let plans = nextest_plans(root, &project, &arguments, "main").unwrap();
    (temp, project, plans.into_iter().next().unwrap())
}

#[test]
#[cfg(unix)]
fn prepared_inventory_and_execution_use_only_the_archive() {
    let (temp, project, plan) = prepared_fixture();
    let root = temp.path();
    let mut environment = environment();
    environment.extend(project.test.lanes["workspace"].env.clone());
    let inventory = plan
        .archive_inventory(
            root,
            &plan,
            &root.join("prepared.tar.zst"),
            root,
            &environment,
        )
        .unwrap();
    assert_eq!(inventory, "demo::tests\tone\n");
    let mut command = plan
        .prepared_command(
            root,
            &plan,
            &PreparedNextest {
                archive_file: &root.join("prepared.tar.zst"),
                workspace_remap: root,
                extract_to: &root.join("consumer"),
                expected_tests: &root.join("expected.tsv"),
                runtime_inputs: &[&root.join("model.bin")],
                environment: &environment,
            },
        )
        .unwrap();
    assert_eq!(command.get_program(), "cargo-nextest");
    let argv = args(&command);
    assert!(argv.windows(2).any(|pair| pair == ["-E", "test(one)"]));
    assert!(argv.contains(&"--profile=default".to_owned()));
    assert!(argv.contains(&"--extract-to".to_owned()));
    assert!(argv.contains(&"--ignore-default-filter".to_owned()));
    assert!(argv.windows(2).any(|pair| pair == ["--test-threads", "2"]));
    assert!(!argv.iter().any(|arg| {
        ["--cargo-profile", "--features", "--workspace", "--timings"].contains(&arg.as_str())
    }));
    assert!(command.status().unwrap().success());
    assert_eq!(children(&temp), "list\nlist\nrun\n");
}

#[test]
#[cfg(unix)]
fn delivery_environment_cannot_change_the_configured_lane_before_listing() {
    for key in ["PATH", "CARGO", "TEST_INVENTORY"] {
        let (temp, _, plan) = prepared_fixture();
        let root = temp.path();
        let mut environment = environment();
        environment.insert(key.to_owned(), "different".to_owned());
        let error = plan
            .archive_inventory(
                root,
                &plan,
                &root.join("prepared.tar.zst"),
                root,
                &environment,
            )
            .unwrap_err();
        assert!(error.to_string().contains(key));
        let error = plan
            .prepared_command(
                root,
                &plan,
                &PreparedNextest {
                    archive_file: &root.join("prepared.tar.zst"),
                    workspace_remap: root,
                    extract_to: &root.join("consumer"),
                    expected_tests: &root.join("expected.tsv"),
                    runtime_inputs: &[],
                    environment: &environment,
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains(key));
        assert_eq!(children(&temp), "");
    }
}

#[test]
fn delivery_environment_uses_native_case_and_unicode_key_equivalence() {
    let directory = TempDir::new().unwrap();
    let root = directory.path();
    let archive = root.join("missing.tar.zst");
    for (configured, alias) in [("PATH", "Path"), ("KITHARA_\u{c5}", "kithara_\u{e5}")] {
        let mut project = synthetic_project();
        project.test.lanes.get_mut("workspace").unwrap().env =
            BTreeMap::from([(configured.to_owned(), "configured".to_owned())]);
        let plan = nextest_plans(root, &project, &[], "main")
            .unwrap()
            .remove(0);
        for value in ["configured", "different"] {
            let environment = BTreeMap::from([(alias.to_owned(), value.to_owned())]);
            let error = plan
                .prepared_command(
                    root,
                    &plan,
                    &PreparedNextest {
                        archive_file: &archive,
                        workspace_remap: root,
                        extract_to: &root.join("consumer"),
                        expected_tests: Path::new("relative.tsv"),
                        runtime_inputs: &[],
                        environment: &environment,
                    },
                )
                .unwrap_err();
            let conflicts = cfg!(windows) && value != "configured";
            assert!(
                error.to_string().contains(if conflicts {
                    "prepared environment conflicts"
                } else {
                    "prepared expected-tests path must be absolute"
                }),
                "{error:#}"
            );
            let error = plan
                .archive_inventory(root, &plan, &archive, root, &environment)
                .unwrap_err();
            assert!(
                error.to_string().contains(if conflicts {
                    "prepared environment conflicts"
                } else {
                    "read prepared archive"
                }),
                "{error:#}"
            );
        }
    }
}

#[test]
#[cfg(unix)]
fn prepared_refuses_an_existing_relative_ledger_before_listing() {
    let (temp, _, plan) = prepared_fixture();
    let root = temp.path();
    let current = std::env::current_dir().unwrap();
    let ledger_directory = TempDir::new_in(&current).unwrap();
    let ledger = ledger_directory
        .path()
        .strip_prefix(&current)
        .unwrap()
        .join("expected.tsv");
    fs::write(&ledger, b"demo::tests\tone\n").unwrap();
    assert!(!ledger.is_absolute() && ledger.is_file());
    let error = plan
        .prepared_command(
            root,
            &plan,
            &PreparedNextest {
                archive_file: &root.join("prepared.tar.zst"),
                workspace_remap: root,
                extract_to: &root.join("consumer"),
                expected_tests: &ledger,
                runtime_inputs: &[],
                environment: &environment(),
            },
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("prepared expected-tests path must be absolute")
    );
    assert_eq!(children(&temp), "");
}

#[test]
#[cfg(unix)]
fn selected_profiles_supply_archive_inputs_for_every_phase() {
    for profile in [
        vec!["--profile", "ci"],
        vec!["--profile=ci"],
        vec!["-P", "ci"],
        vec!["-Pci"],
    ] {
        let (temp, project, _) = prepared_fixture();
        let root = temp.path();
        let archive = root.join("prepared.tar.zst");
        let included = root.join("profile-only.txt");
        fs::write(&included, "archive\nprofile-specific runtime input\n").unwrap();
        fs::write(root.join("nextest.toml"),
            "[profile.ci.archive]\ninclude = [{path = 'profile-only.txt', relative-to = 'workspace'}]\n").unwrap();
        fs::remove_file(&archive).unwrap();
        let mut arguments = vec!["--config-file=nextest.toml".to_owned()];
        arguments.extend(profile.into_iter().map(str::to_owned));
        let plan = nextest_plans(root, &project, &arguments, "main")
            .unwrap()
            .remove(0);
        let mut environment = environment();
        environment.extend([
            ("TEST_REQUIRED_PROFILE".to_owned(), "ci".to_owned()),
            (
                "TEST_PROFILE_CONFIG".to_owned(),
                root.join("nextest.toml").display().to_string(),
            ),
            (
                "TEST_PROFILE_INPUT".to_owned(),
                included.display().to_string(),
            ),
        ]);
        assert!(
            plan.archive_command(&archive)
                .unwrap()
                .envs(&environment)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(fs::read(&archive).unwrap(), fs::read(&included).unwrap());
        assert_eq!(
            plan.archive_inventory(root, &plan, &archive, root, &environment)
                .unwrap(),
            "demo::tests\tone\n"
        );
        let mut command = plan
            .prepared_command(
                root,
                &plan,
                &PreparedNextest {
                    archive_file: &archive,
                    workspace_remap: root,
                    extract_to: &root.join("consumer"),
                    expected_tests: &root.join("expected.tsv"),
                    runtime_inputs: &[&included],
                    environment: &environment,
                },
            )
            .unwrap();
        assert!(command.status().unwrap().success());
        assert_eq!(children(&temp), "archive\nlist\nlist\nrun\n");
    }
}

#[test]
#[cfg(unix)]
fn prepared_inventory_strips_supported_jobs_options_without_changing_run_arguments() {
    for jobs in [
        vec!["-j-1"],
        vec!["-j=-1"],
        vec!["-j", "-1"],
        vec!["-j+1"],
        vec!["-jnum-cpus"],
        vec!["--jobs=-1"],
        vec!["--jobs", "-1"],
        vec!["--test-threads=-1"],
    ] {
        let (temp, project, _) = prepared_fixture();
        let root = temp.path();
        let arguments = jobs.into_iter().map(str::to_owned).collect::<Vec<_>>();
        let ordinary =
            super::nextest_command_for_lane(&project, "workspace", &arguments, NextestAction::Run)
                .unwrap();
        assert!(args(&ordinary).ends_with(&arguments));
        let plan = nextest_plans(root, &project, &arguments, "main")
            .unwrap()
            .remove(0);
        let mut command = plan
            .prepared_command(
                root,
                &plan,
                &PreparedNextest {
                    archive_file: &root.join("prepared.tar.zst"),
                    workspace_remap: root,
                    extract_to: &root.join("consumer"),
                    expected_tests: &root.join("expected.tsv"),
                    runtime_inputs: &[],
                    environment: &environment(),
                },
            )
            .unwrap();
        assert!(args(&command).ends_with(&arguments));
        assert!(command.status().unwrap().success());
        assert_eq!(children(&temp), "list\nrun\n");
    }
}

#[test]
#[cfg(unix)]
fn prepared_refuses_foreign_plans_before_starting_any_child() {
    for (field, value) in [
        ("features", serde_json::json!(["other"])),
        (
            "cargo",
            serde_json::json!({"workspace": true, "profile": "other"}),
        ),
        ("lane", serde_json::json!("other")),
        (
            "runner",
            serde_json::json!({"nextest": {"filter": "test(other)"}}),
        ),
    ] {
        let (temp, _, plan) = prepared_fixture();
        let root = temp.path();
        let mut recorded = serde_json::to_value(&plan).unwrap();
        recorded["lane"][field] = value;
        let recorded: NextestPlan = serde_json::from_value(recorded).unwrap();
        plan.archive_inventory(
            root,
            &recorded,
            &root.join("prepared.tar.zst"),
            root,
            &environment(),
        )
        .unwrap_err();
        plan.prepared_command(
            root,
            &recorded,
            &PreparedNextest {
                archive_file: &root.join("prepared.tar.zst"),
                workspace_remap: root,
                extract_to: &root.join("consumer"),
                expected_tests: &root.join("expected.tsv"),
                runtime_inputs: &[],
                environment: &environment(),
            },
        )
        .unwrap_err();
        assert_eq!(children(&temp), "");
    }
}

#[test]
#[cfg(unix)]
fn prepared_refuses_missing_inputs_corrupt_archives_and_wrong_inventory_without_running() {
    for failure in [
        "archive",
        "workspace",
        "expected",
        "runtime",
        "corrupt",
        "inventory",
        "json",
    ] {
        let (temp, _, plan) = prepared_fixture();
        let root = temp.path();
        match failure {
            "archive" => fs::remove_file(root.join("prepared.tar.zst")).unwrap(),
            "expected" => fs::remove_file(root.join("expected.tsv")).unwrap(),
            "runtime" => fs::remove_file(root.join("model.bin")).unwrap(),
            "corrupt" => fs::write(root.join("prepared.tar.zst"), "corrupt\n").unwrap(),
            "inventory" => fs::write(root.join("expected.tsv"), "demo::tests\tother\n").unwrap(),
            "json" => fs::write(root.join("selected.json"), "bad json").unwrap(),
            _ => {}
        }
        plan.prepared_command(
            root,
            &plan,
            &PreparedNextest {
                archive_file: &root.join("prepared.tar.zst"),
                workspace_remap: if failure == "workspace" {
                    Path::new("/missing/prepared-workspace")
                } else {
                    root
                },
                extract_to: &root.join("consumer"),
                expected_tests: &root.join("expected.tsv"),
                runtime_inputs: &[&root.join("model.bin")],
                environment: &environment(),
            },
        )
        .unwrap_err();
        assert_eq!(
            children(&temp),
            if ["corrupt", "inventory", "json"].contains(&failure) {
                "list\n"
            } else {
                ""
            }
        );
    }
}
