use std::{collections::BTreeSet, fs, path::Path};

use serde_yaml_ng::{Mapping, Value};

const CHECKOUT: &str = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1";
const UPLOAD_ARTIFACT: &str = "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a";

const AUTHORIZATION_SCRIPT: &str = r#"python3 - <<'PY'
import json
import os
import sys

actor = os.environ["ACTOR"]
owner = os.environ["OWNER"]
if actor != owner:
    print(f"CI may only be started by repository owner {owner!r}, got {actor!r}")
    sys.exit(1)

raw_labels = os.environ.get("RUNNER_LABELS", "")
try:
    labels = json.loads(raw_labels)
except json.JSONDecodeError as error:
    print(f"KITHARA_RUNNER_LABELS is not valid JSON: {error}")
    sys.exit(1)
if not isinstance(labels, list) or not labels or not all(
    isinstance(label, str) and label for label in labels
):
    print("KITHARA_RUNNER_LABELS must be a non-empty JSON array of non-empty strings")
    sys.exit(1)
PY"#;

const REQUIRED_SCRIPT: &str = r#"python3 - <<'PY'
import json
import os
import sys

results = json.loads(os.environ["RESULTS"])
incomplete = {
    name: job["result"]
    for name, job in results.items()
    if job["result"] != "success"
}
if incomplete:
    print(f"required CI jobs did not execute successfully: {incomplete}")
    sys.exit(1)
PY"#;

fn github_workflow(name: &str) -> Value {
    let text = github_workflow_text(name);
    serde_yaml_ng::from_str(&text).expect("workflow is valid YAML")
}

fn github_workflow_text(name: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a workspace root");
    fs::read_to_string(root.join(".github/workflows").join(name)).expect("workflow is readable")
}

fn mapping_field<'a>(mapping: &'a Mapping, name: &str) -> &'a Value {
    mapping
        .get(name)
        .unwrap_or_else(|| panic!("YAML mapping has no `{name}` field"))
}

fn workflow_jobs(workflow: &Value) -> &Mapping {
    mapping_field(
        workflow.as_mapping().expect("workflow is a mapping"),
        "jobs",
    )
    .as_mapping()
    .expect("workflow jobs are a mapping")
}

fn workflow_job<'a>(jobs: &'a Mapping, name: &str) -> &'a Mapping {
    mapping_field(jobs, name)
        .as_mapping()
        .unwrap_or_else(|| panic!("workflow job `{name}` must be a mapping"))
}

fn workflow_job_names(jobs: &Mapping) -> BTreeSet<String> {
    jobs.keys()
        .map(|name| {
            name.as_str()
                .expect("workflow job name is a string")
                .to_owned()
        })
        .collect()
}

fn job_needs(job: &Mapping) -> BTreeSet<String> {
    match mapping_field(job, "needs") {
        Value::String(name) => BTreeSet::from([name.clone()]),
        Value::Sequence(names) => names
            .iter()
            .map(|name| {
                name.as_str()
                    .expect("workflow dependency is a string")
                    .to_owned()
            })
            .collect(),
        _ => panic!("workflow job dependencies must be a string or sequence"),
    }
}

fn first_step(job: &Mapping) -> &Mapping {
    mapping_field(job, "steps")
        .as_sequence()
        .expect("workflow steps are a sequence")
        .first()
        .expect("workflow has at least one step")
        .as_mapping()
        .expect("workflow step is a mapping")
}

fn named_step<'a>(job: &'a Mapping, name: &str) -> &'a Mapping {
    mapping_field(job, "steps")
        .as_sequence()
        .expect("workflow steps are a sequence")
        .iter()
        .find_map(|step| {
            let step = step.as_mapping()?;
            (step.get("name").and_then(Value::as_str) == Some(name)).then_some(step)
        })
        .unwrap_or_else(|| panic!("workflow job has no `{name}` step"))
}

fn assert_always(step: &Mapping) {
    assert_eq!(mapping_field(step, "if").as_str(), Some("always()"));
}

fn assert_uploads(step: &Mapping, paths: &[&str]) {
    assert_always(step);
    assert_eq!(mapping_field(step, "uses").as_str(), Some(UPLOAD_ARTIFACT));
    let with = mapping_field(step, "with")
        .as_mapping()
        .expect("artifact inputs are a mapping");
    let actual: BTreeSet<&str> = mapping_field(with, "path")
        .as_str()
        .expect("artifact paths are a string")
        .lines()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .collect();
    assert_eq!(actual, paths.iter().copied().collect());
}

fn assert_no_key(value: &Value, forbidden: &str) {
    match value {
        Value::Sequence(values) => {
            for value in values {
                assert_no_key(value, forbidden);
            }
        }
        Value::Mapping(mapping) => {
            for (key, value) in mapping {
                assert_ne!(
                    key.as_str(),
                    Some(forbidden),
                    "workflow contains forbidden `{forbidden}`"
                );
                assert_no_key(value, forbidden);
            }
        }
        _ => {}
    }
}

fn assert_hosted_authorization(job: &Mapping) {
    assert_eq!(
        mapping_field(job, "runs-on").as_str(),
        Some("ubuntu-latest")
    );
    assert!(!job.contains_key("if"));

    let step = first_step(job);
    let env = mapping_field(step, "env")
        .as_mapping()
        .expect("authorization environment is a mapping");
    assert_eq!(
        mapping_field(env, "ACTOR").as_str(),
        Some("${{ github.actor }}")
    );
    assert_eq!(
        mapping_field(env, "OWNER").as_str(),
        Some("${{ github.repository_owner }}")
    );
    assert_eq!(
        mapping_field(env, "RUNNER_LABELS").as_str(),
        Some("${{ vars.KITHARA_RUNNER_LABELS }}")
    );
    assert_eq!(
        mapping_field(step, "run")
            .as_str()
            .expect("authorization step is a script")
            .trim(),
        AUTHORIZATION_SCRIPT
    );
}

#[test]
fn github_ci_is_fail_closed_and_aggregates_every_job() {
    let workflow = github_workflow("ci.yml");
    let jobs = workflow_jobs(&workflow);

    let advisory_browser = workflow_job(jobs, "wasm-browser");
    assert_eq!(
        mapping_field(advisory_browser, "name").as_str(),
        Some("Chromium WebCodecs (advisory)")
    );
    assert_eq!(
        mapping_field(advisory_browser, "continue-on-error").as_bool(),
        Some(true)
    );

    assert_hosted_authorization(workflow_job(jobs, "authorize"));
    let gate = workflow_job(jobs, "gate");
    assert_eq!(job_needs(gate), BTreeSet::from(["authorize".to_owned()]));
    assert_eq!(
        mapping_field(gate, "runs-on").as_str(),
        Some("${{ fromJSON(vars.KITHARA_RUNNER_LABELS) }}")
    );

    for name in workflow_job_names(jobs) {
        if matches!(name.as_str(), "authorize" | "gate" | "required") {
            continue;
        }
        let job = workflow_job(jobs, &name);
        if name != "wasm-browser" {
            assert_no_key(&Value::Mapping(job.clone()), "continue-on-error");
        }
        assert_eq!(
            job_needs(job),
            BTreeSet::from(["gate".to_owned()]),
            "workflow job `{name}` bypasses the self-hosted gate"
        );
        let condition = job.get("if").and_then(Value::as_str).unwrap_or_default();
        assert!(!condition.contains("KITHARA_RUNNER_LABELS"));
        assert!(!condition.contains("github.actor"));
    }

    let required = workflow_job(jobs, "required");
    assert_eq!(
        mapping_field(required, "runs-on").as_str(),
        Some("ubuntu-latest")
    );
    assert_eq!(
        mapping_field(required, "if").as_str(),
        Some("${{ always() }}")
    );
    let mut expected = workflow_job_names(jobs);
    expected.remove("required");
    expected.remove("wasm-browser");
    assert_eq!(job_needs(required), expected);

    let step = first_step(required);
    let env = mapping_field(step, "env")
        .as_mapping()
        .expect("required environment is a mapping");
    assert_eq!(
        mapping_field(env, "RESULTS").as_str(),
        Some("${{ toJSON(needs) }}")
    );
    assert_eq!(
        mapping_field(step, "run")
            .as_str()
            .expect("required step is a script")
            .trim(),
        REQUIRED_SCRIPT
    );
}

#[test]
fn standalone_rtsan_is_fail_closed_before_expanding_every_lane() {
    let workflow = github_workflow("rtsan.yml");
    assert_no_key(&workflow, "continue-on-error");
    let jobs = workflow_jobs(&workflow);

    assert_hosted_authorization(workflow_job(jobs, "authorize"));
    let rtsan = workflow_job(jobs, "rtsan");
    assert_eq!(job_needs(rtsan), BTreeSet::from(["authorize".to_owned()]));
    assert!(!rtsan.contains_key("if"));

    let strategy = mapping_field(rtsan, "strategy")
        .as_mapping()
        .expect("RTSan strategy is a mapping");
    let matrix = mapping_field(strategy, "matrix")
        .as_mapping()
        .expect("RTSan matrix is a mapping");
    let lanes: Vec<&str> = mapping_field(matrix, "lane")
        .as_sequence()
        .expect("RTSan lanes are a sequence")
        .iter()
        .map(|lane| lane.as_str().expect("RTSan lane is a string"))
        .collect();
    assert_eq!(lanes, ["rtsan", "rtsan-file", "rtsan-hls"]);
}

#[test]
fn stress_workflow_keeps_the_first_failure_and_collects_its_evidence() {
    let workflow = github_workflow("stress.yml");
    assert_no_key(&workflow, "continue-on-error");
    let root = workflow.as_mapping().expect("workflow is a mapping");
    let triggers = mapping_field(root, "on")
        .as_mapping()
        .expect("workflow triggers are a mapping");

    for trigger in ["workflow_call", "workflow_dispatch"] {
        let inputs = mapping_field(
            mapping_field(triggers, trigger)
                .as_mapping()
                .unwrap_or_else(|| panic!("`{trigger}` is a mapping")),
            "inputs",
        )
        .as_mapping()
        .expect("workflow inputs are a mapping");
        assert_eq!(
            inputs
                .keys()
                .map(|name| name.as_str().expect("input name is a string"))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["count", "filter", "revision"])
        );
        for name in ["revision", "filter", "count"] {
            let input = mapping_field(inputs, name)
                .as_mapping()
                .expect("input definition is a mapping");
            assert_eq!(mapping_field(input, "type").as_str(), Some("string"));
            assert_eq!(mapping_field(input, "required").as_bool(), Some(false));
        }
        assert_eq!(
            mapping_field(
                mapping_field(inputs, "filter")
                    .as_mapping()
                    .expect("filter input is a mapping"),
                "default",
            )
            .as_str(),
            Some("all()")
        );
        assert_eq!(
            mapping_field(
                mapping_field(inputs, "count")
                    .as_mapping()
                    .expect("count input is a mapping"),
                "default",
            )
            .as_str(),
            Some("50")
        );
    }

    let job = workflow_job(workflow_jobs(&workflow), "stress");
    assert_eq!(
        mapping_field(job, "runs-on").as_str(),
        Some("${{ fromJSON(vars.KITHARA_RUNNER_LABELS) }}")
    );
    let guard = mapping_field(job, "if")
        .as_str()
        .expect("stress job has an authorization guard");
    for condition in [
        "github.event.repository.fork == true",
        "github.actor == github.repository_owner",
        "vars.KITHARA_RUNNER_LABELS != ''",
    ] {
        assert!(
            guard.contains(condition),
            "stress guard omits `{condition}`"
        );
    }

    let controller = named_step(job, "Checkout controller");
    assert_eq!(mapping_field(controller, "uses").as_str(), Some(CHECKOUT));
    let controller_inputs = mapping_field(controller, "with")
        .as_mapping()
        .expect("checkout inputs are a mapping");
    assert_eq!(
        mapping_field(controller_inputs, "ref").as_str(),
        Some("${{ github.sha }}")
    );
    assert_eq!(
        mapping_field(controller_inputs, "path").as_str(),
        Some("controller")
    );

    let subject = named_step(job, "Checkout subject");
    assert_eq!(mapping_field(subject, "uses").as_str(), Some(CHECKOUT));
    let subject_inputs = mapping_field(subject, "with")
        .as_mapping()
        .expect("checkout inputs are a mapping");
    assert_eq!(
        mapping_field(subject_inputs, "ref").as_str(),
        Some("${{ inputs.revision || github.sha }}")
    );
    assert_eq!(
        mapping_field(subject_inputs, "path").as_str(),
        Some("subject")
    );

    let primary = named_step(job, "Repeat the selection");
    assert_eq!(
        mapping_field(primary, "working-directory").as_str(),
        Some("subject")
    );
    assert_eq!(
        mapping_field(primary, "run").as_str(),
        Some(
            "just test run --profile stress --config-file \"$GITHUB_WORKSPACE/controller/.config/nextest.toml\" -E \"$FILTER\" --stress-count \"$COUNT\""
        )
    );
    assert!(
        !mapping_field(primary, "run")
            .as_str()
            .expect("stress command is a string")
            .contains("retries")
    );

    let report = named_step(job, "Summarize the stress evidence");
    assert_always(report);
    assert_eq!(
        mapping_field(report, "working-directory").as_str(),
        Some("controller")
    );
    let report_script = mapping_field(report, "run")
        .as_str()
        .expect("stress report is a script");
    for contract in [
        "just tooling xtask stress-report --allow-missing --expected-count \"$COUNT\"",
        "--junit \"$GITHUB_WORKSPACE/subject/target/nextest/stress/junit.xml\"",
        "--output \"$GITHUB_WORKSPACE/subject/target/stress-report.md\"",
        "$GITHUB_STEP_SUMMARY",
        "$REVISION",
        "$FILTER",
    ] {
        assert!(
            report_script.contains(contract),
            "stress report omits `{contract}`"
        );
    }

    assert_uploads(
        named_step(job, "Upload the stress evidence"),
        &[
            "subject/target/nextest/stress/junit.xml",
            "subject/target/stress-report.md",
        ],
    );
}

#[test]
fn stress_profile_records_failures_without_retries_or_timeout_inflation() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a workspace root");
    let nextest: toml::Value = toml::from_str(
        &fs::read_to_string(root.join(".config/nextest.toml")).expect("nextest config is readable"),
    )
    .expect("nextest config is valid TOML");
    let stress = nextest["profile"]["stress"]
        .as_table()
        .expect("stress profile is a table");

    assert!(!stress.contains_key("retries"));
    let timeout = stress["slow-timeout"]
        .as_table()
        .expect("stress timeout is a table");
    assert_eq!(timeout["period"].as_str(), Some("120s"));
    assert_eq!(timeout["terminate-after"].as_integer(), Some(1));

    let junit = stress["junit"].as_table().expect("stress JUnit is a table");
    assert_eq!(junit["path"].as_str(), Some("junit.xml"));
    assert_eq!(junit["store-failure-output"].as_bool(), Some(true));
}

#[test]
fn nightly_collector_is_read_only_and_does_not_mirror_the_source_verdict() {
    let workflow = github_workflow("nightly-report.yml");
    assert_no_key(&workflow, "continue-on-error");
    let root = workflow.as_mapping().expect("workflow is a mapping");
    let permissions = mapping_field(root, "permissions")
        .as_mapping()
        .expect("permissions are a mapping");
    assert_eq!(
        permissions
            .keys()
            .map(|name| name.as_str().expect("permission name is a string"))
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["actions", "contents"])
    );
    for permission in ["actions", "contents"] {
        assert_eq!(
            mapping_field(permissions, permission).as_str(),
            Some("read")
        );
    }

    let job = workflow_job(workflow_jobs(&workflow), "report");
    assert_eq!(
        mapping_field(job, "runs-on").as_str(),
        Some("ubuntu-latest")
    );
    assert!(!job.contains_key("if"));

    let collect = named_step(job, "Collect the source run jobs");
    let script = mapping_field(collect, "run")
        .as_str()
        .expect("nightly collector is a script");
    for contract in [
        "gh api \"repos/$REPOSITORY/actions/runs/$RUN_ID/jobs\"",
        "target/nightly-report.md",
        "$GITHUB_STEP_SUMMARY",
    ] {
        assert!(
            script.contains(contract),
            "nightly report omits `{contract}`"
        );
    }
    assert!(!script.contains("exit 1"));

    assert_uploads(
        named_step(job, "Upload the nightly report"),
        &["target/nightly-report.md"],
    );

    let text = github_workflow_text("nightly-report.yml");
    for forbidden in ["gh issue", "issues: write"] {
        assert!(
            !text.contains(forbidden),
            "nightly collector contains forbidden `{forbidden}`"
        );
    }
}
