use std::{
    fs::{self, File},
    path::Path,
    process::Command,
};

use serde_json::Value;
use tempfile::TempDir;

use crate::{
    junit::parse_junit,
    stress_report::{self, StressReportArgs},
    test::repository_tests::root,
};

fn fixture() -> TempDir {
    let temp = tempfile::tempdir().expect("temporary ignored-test fixture");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nextest-ignored");
    for name in ["Cargo.toml", "ignored.rs"] {
        fs::copy(source.join(name), temp.path().join(name)).expect("copy ignored-test fixture");
    }
    let repository: toml::Value = toml::from_str(
        &fs::read_to_string(root().join(".config/nextest.toml")).expect("nextest configuration"),
    )
    .expect("nextest TOML");
    let profiles = toml::Table::from_iter([(
        "stress".to_owned(),
        repository["profile"]["stress"].clone(),
    )]);
    let config = toml::Table::from_iter([("profile".to_owned(), toml::Value::Table(profiles))]);
    fs::write(
        temp.path().join("nextest.toml"),
        toml::to_string(&config).expect("isolated stress TOML"),
    )
    .expect("write isolated stress profile");
    temp
}

fn nextest(temp: &Path, action: &str, ignored: &str) -> Command {
    let cargo = std::env::var_os("CARGO").expect("cargo supplied by the test runner");
    let mut command = Command::new(cargo);
    command
        .current_dir(temp)
        .args(["nextest", action, "--offline", "--profile", "stress"])
        .arg("--config-file")
        .arg(temp.join("nextest.toml"))
        .args(["--run-ignored", ignored])
        .env("CARGO_TARGET_DIR", temp.join("build"))
        .env("IGNORED_ATTEMPT_PATH", temp.join("flake-attempt"));
    command
}

#[test]
fn selected_ignored_attempts_are_kept_in_compiled_inventory_and_report() {
    let temp = fixture();
    let inventory = temp.path().join("inventory.json");
    let list = nextest(temp.path(), "list", "only")
        .args(["-E", "test(=pinned_red) + test(=declared_flake)"])
        .args(["--message-format", "json"])
        .stdout(File::create(&inventory).expect("inventory file"))
        .output()
        .expect("list the compiled ignored tests");
    assert!(list.status.success(), "{}", String::from_utf8_lossy(&list.stderr));
    let listed: Value = serde_json::from_str(&fs::read_to_string(&inventory).expect("inventory"))
        .expect("nextest inventory JSON");
    let cases = &listed["rust-suites"]["nextest-ignored-contract::ignored"]["testcases"];
    for name in ["pinned_red", "declared_flake"] {
        assert_eq!(cases[name]["ignored"], true, "actual ignored flag for {name}");
        assert_eq!(
            cases[name]["filter-match"]["status"],
            "matches",
            "the explicit ignored selection must include {name}"
        );
    }
    assert_eq!(cases["manual_entrypoint"]["filter-match"]["status"], "mismatch");

    let run = nextest(temp.path(), "run", "only")
        .args(["-E", "test(=pinned_red) + test(=declared_flake)"])
        .args(["--stress-count", "2", "--test-threads", "1"])
        .output()
        .expect("run ignored attempts");
    assert!(!run.status.success(), "the deliberate red fixture must fail nextest");
    let junit = temp.path().join("target/nextest/stress/junit.xml");
    let xml = fs::read_to_string(&junit).expect("actual ignored JUnit");
    let cases = parse_junit(&xml).expect("parse actual ignored attempts");
    assert_eq!(cases.len(), 4, "both selected tests execute twice: {xml}");
    let red = cases.iter().filter(|case| case.name == "pinned_red").collect::<Vec<_>>();
    let flake = cases.iter().filter(|case| case.name == "declared_flake").collect::<Vec<_>>();
    assert_eq!(red.len(), 2);
    assert!(red.iter().all(|case| case.failed));
    assert_eq!(flake.len(), 2);
    assert_eq!(flake.iter().filter(|case| case.failed).count(), 1);
    assert_eq!(flake.iter().filter(|case| !case.failed).count(), 1);
    let report = stress_report::lane_report(&StressReportArgs::new(
        junit,
        inventory,
        temp.path().join("ignored-report.md"),
        2,
    ))
    .expect("a report must retain explicitly selected ignored attempts");
    assert!(report.verdict.is_err(), "known reds are not primary clean evidence");
    assert!(report.markdown.contains("pinned_red"), "{}", report.markdown);
    assert!(report.markdown.contains("declared_flake"), "{}", report.markdown);
    assert!(report.markdown.contains("50.00%"), "{}", report.markdown);
    assert!(!report.markdown.contains("manual_entrypoint"), "{}", report.markdown);
}

#[test]
fn ordinary_compiled_inventory_still_skips_ignored_functions() {
    let temp = fixture();
    let inventory = temp.path().join("inventory.json");
    let list = nextest(temp.path(), "list", "default")
        .args(["--message-format", "json"])
        .stdout(File::create(&inventory).expect("inventory file"))
        .output()
        .expect("list ordinary tests");
    assert!(list.status.success(), "{}", String::from_utf8_lossy(&list.stderr));
    let run = nextest(temp.path(), "run", "default")
        .args(["--stress-count", "1", "--test-threads", "1"])
        .output()
        .expect("run ordinary fixture tests");
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let report = stress_report::lane_report(&StressReportArgs::new(
        temp.path().join("target/nextest/stress/junit.xml"),
        inventory,
        temp.path().join("ordinary-report.md"),
        1,
    ))
    .expect("ordinary report");
    assert!(report.verdict.is_ok(), "{}", report.markdown);
    assert!(report.markdown.contains("ordinary_pass"), "{}", report.markdown);
    assert!(!report.markdown.contains("pinned_red"), "{}", report.markdown);
}
