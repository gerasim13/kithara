#[cfg(unix)]
use std::process::Command;
use std::{fs, path::Path};

use serde_yaml_ng::Value;

fn workflow(name: &str) -> Value {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let source = fs::read_to_string(root.join(".github/workflows").join(name)).unwrap();
    serde_yaml_ng::from_str(&source).unwrap()
}

fn step<'a>(job: &'a Value, name: &str) -> &'a Value {
    job["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["name"].as_str() == Some(name))
        .unwrap()
}

#[test]
fn a_test_filter_reaches_each_canonical_lane_call_as_data() {
    let dispatch = workflow("dispatch.yml");
    assert_eq!(
        dispatch["on"]["workflow_dispatch"]["inputs"]["test-filter"]["type"].as_str(),
        Some("string")
    );
    for role in ["gate", "platforms", "deep", "mutants", "quality"] {
        assert_eq!(
            dispatch["jobs"][role]["with"]["test-filter"].as_str(),
            Some("${{ inputs.test-filter || '' }}")
        );
        assert_eq!(dispatch["jobs"][role]["needs"].as_str(), Some("authorize"));
    }
    let fan_out = workflow("run.yml");
    for job in ["run", "dependent"] {
        assert_eq!(
            fan_out["jobs"][job]["with"]["test-filter"].as_str(),
            Some("${{ inputs.test-filter }}")
        );
    }
    let lane = workflow("lane.yml");
    let command = step(&lane["jobs"]["run"], "Run the lane");
    assert_eq!(
        command["env"]["TEST_FILTER"].as_str(),
        Some("${{ inputs.test-filter }}")
    );
    let script = command["run"].as_str().unwrap();
    assert!(script.contains("args+=(--test-filter \"$TEST_FILTER\")"));
    assert!(script.contains("just ci lane \"${args[@]}\""));
    assert!(!script.contains("${{ inputs."));
    assert!(script.contains("set -euo pipefail"));
    assert!(script.contains("status=${PIPESTATUS[0]}"));
    assert!(script.contains("exit \"$status\""));
}

#[test]
fn direct_and_reusable_lane_routes_share_authorization_and_current_sha() {
    let lane = workflow("lane.yml");
    assert_eq!(
        lane["on"]["workflow_dispatch"]["inputs"]["resolve"]["default"].as_bool(),
        Some(true)
    );
    assert_eq!(
        lane["on"]["workflow_call"]["inputs"]["resolve"]["default"].as_bool(),
        Some(false)
    );
    assert_eq!(lane["permissions"]["contents"].as_str(), Some("read"));
    let jobs = &lane["jobs"];
    let authorization = step(&jobs["authorize"], "Validate repository owner and request");
    let script = authorization["run"].as_str().unwrap();
    for guard in [
        "\"$ACTOR\" == \"$OWNER\"",
        "\"$TRIGGERING_ACTOR\" == \"$OWNER\"",
        "refs/heads/*",
        "^[0-9a-fA-F]{40}$",
        "an unresolved call needs rendered lane metadata",
    ] {
        assert!(script.contains(guard), "missing guard: {guard}");
    }
    for job in ["select", "run"] {
        let checkout = step(&jobs[job], "Checkout code");
        assert_eq!(checkout["with"]["ref"].as_str(), Some("${{ github.sha }}"));
    }
    let admission = jobs["run"]["if"].as_str().unwrap();
    assert!(admission.contains("needs.authorize.result == 'success'"));
    assert!(!admission.contains("github.event_name"));
}

#[test]
fn a_direct_lane_uses_catalog_metadata_without_the_heavy_role_queue() {
    let lane = workflow("lane.yml");
    let jobs = &lane["jobs"];
    assert_eq!(jobs["select"]["needs"].as_str(), Some("authorize"));
    let render = step(&jobs["select"], "Resolve one catalog entry");
    let script = render["run"].as_str().unwrap();
    assert!(script.contains(
        "just ci lanes --role \"$ROLE\" --kind \"$KIND\" --only \"$LANE\" --field matrix"
    ));
    assert!(script.contains("\"$matrix\" != '[]'"));
    assert_eq!(jobs["run"]["concurrency"], Value::Null);
    for (field, metadata) in [("runs-on", ".runner"), ("timeout-minutes", ".timeout")] {
        assert!(jobs["run"][field].as_str().unwrap().contains(metadata));
    }
    let checkout = step(&jobs["run"], "Checkout code");
    assert!(
        checkout["with"]["fetch-depth"]
            .as_str()
            .unwrap()
            .contains(".depth")
    );
    for field in [
        "LANE_ARTIFACT_NAME",
        "LANE_ARTIFACT_PATH",
        "LANE_ARTIFACT_WHEN",
    ] {
        assert!(
            jobs["run"]["env"][field]
                .as_str()
                .unwrap()
                .contains(".artifact.")
        );
    }
}

#[test]
fn focused_evidence_preserves_the_harness_log_and_junit_on_failure() {
    let lane = workflow("lane.yml");
    let evidence = step(&lane["jobs"]["run"], "Upload focused test evidence");
    assert!(evidence["if"].as_str().unwrap().contains("always()"));
    let paths = evidence["with"]["path"].as_str().unwrap();
    for path in [
        "lane-sha.txt",
        "lane-request.txt",
        "lane.log",
        "lane-exit-status.txt",
        "target/nextest/**/junit.xml",
    ] {
        assert!(paths.contains(path));
    }
}

#[test]
fn self_hosted_admission_checks_the_current_actor_pair_before_scheduling() {
    for (name, jobs) in [
        ("lane.yml", vec!["select", "run"]),
        ("run.yml", vec!["select"]),
    ] {
        let source = workflow(name);
        for job in jobs {
            let admission = source["jobs"][job]["if"].as_str().unwrap();
            assert!(
                admission.contains(
                    "github.actor == github.repository_owner && github.triggering_actor == github.repository_owner"
                ),
                "{name}/{job} must require both current actors before scheduling"
            );
        }
    }
    let lane = workflow("lane.yml");
    let executor = &lane["jobs"]["run"];
    let admission = executor["if"].as_str().unwrap();
    for existing in [
        "!cancelled()",
        "needs.authorize.result == 'success'",
        "needs.select.result == 'success'",
        "needs.select.result == 'skipped'",
    ] {
        assert!(admission.contains(existing));
    }
    assert_eq!(
        executor["needs"],
        Value::Sequence(vec![
            Value::String("authorize".to_owned()),
            Value::String("select".to_owned()),
        ])
    );
    assert!(
        lane["jobs"]["select"]["if"]
            .as_str()
            .unwrap()
            .contains("inputs.resolve")
    );
}

#[cfg(unix)]
#[test]
fn hosted_authorization_accepts_legal_branches_and_rejects_main_review_or_all() {
    let lane = workflow("lane.yml");
    let authorization = step(
        &lane["jobs"]["authorize"],
        "Validate repository owner and request",
    );
    assert_eq!(
        authorization["env"]["DEFAULT_BRANCH"].as_str(),
        Some("${{ github.event.repository.default_branch }}")
    );
    let script = authorization["run"].as_str().unwrap();
    for resolve in ["true", "false"] {
        for (requested, kind, reference, accepted, reason) in [
            ("linux-tooling", "nightly", "refs/heads/review", true, ""),
            ("linux-tooling", "main", "refs/heads/main", true, ""),
            (
                "all",
                "nightly",
                "refs/heads/review",
                false,
                "name one lane",
            ),
            (
                "linux-tooling",
                "main",
                "refs/heads/review",
                false,
                "main lanes require the repository default branch",
            ),
        ] {
            let output = Command::new("bash")
                .args(["--noprofile", "--norc", "-c", script])
                .envs([
                    ("ACTOR", "owner"),
                    ("TRIGGERING_ACTOR", "owner"),
                    ("OWNER", "owner"),
                    ("REF", reference),
                    ("DEFAULT_BRANCH", "main"),
                    ("SHA", "0123456789012345678901234567890123456789"),
                    ("LANE", requested),
                    ("KIND", kind),
                    ("RESOLVE", resolve),
                    ("TIMEOUT", "10"),
                    ("RUNNER_LABELS", "[\"self-hosted\",\"linux\"]"),
                ])
                .output()
                .expect("run the actual hosted authorization script");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                output.status.success(),
                accepted,
                "resolve={resolve} lane={requested} kind={kind} ref={reference}: {stdout}{stderr}"
            );
            if !accepted {
                assert!(
                    stdout.contains(reason),
                    "the rejection must name {reason}: {stdout}{stderr}"
                );
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn hosted_authorization_preserves_runner_label_schema_validation() {
    let lane = workflow("lane.yml");
    let authorization = step(
        &lane["jobs"]["authorize"],
        "Validate repository owner and request",
    );
    let script = authorization["run"].as_str().unwrap();
    for labels in ["invalid-json", "[]", "{}", "[1]", "[\"\"]"] {
        let output = Command::new("bash")
            .args(["--noprofile", "--norc", "-c", script])
            .envs([
                ("ACTOR", "owner"),
                ("TRIGGERING_ACTOR", "owner"),
                ("OWNER", "owner"),
                ("REF", "refs/heads/review"),
                ("DEFAULT_BRANCH", "main"),
                ("SHA", "0123456789012345678901234567890123456789"),
                ("LANE", "linux-tooling"),
                ("KIND", "nightly"),
                ("RESOLVE", "true"),
                ("TIMEOUT", "10"),
                ("RUNNER_LABELS", labels),
            ])
            .output()
            .expect("run the actual hosted authorization script");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !output.status.success(),
            "invalid runner labels passed: {labels}"
        );
        assert!(stdout.contains("KITHARA_RUNNER_LABELS"), "{stdout}");
    }
}
