use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use serde_yaml_ng::{Mapping, Value};
use syn::{
    BinOp, Expr, ItemConst, Meta, Stmt, Token, parse::Parser, punctuated::Punctuated, visit::Visit,
};

const CHECKOUT: &str = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1";
const DOWNLOAD_ARTIFACT: &str =
    "actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c";
const INSTALL_ACTION: &str = "taiki-e/install-action@b20dedce73af6905cdc30d6611090c9b67557c8d";
const UPLOAD_ARTIFACT: &str = "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a";
const STRESS_RAW_DIR: &str =
    "${{ runner.temp }}/stress-raw-${{ github.run_id }}-${{ github.run_attempt }}";
const REPRO_RUST_LOG: &str = "warn";
const DIAGNOSTIC_RUST_LOG: &str = "warn,flash::hang=debug,kithara_platform::no_block=debug,kithara_queue=debug,kithara_hls=debug,kithara_stream=debug,kithara_net=debug,kithara_audio=debug";
const STRESS_MAX_DECLARED_TIMEOUT_SECS: u64 = 600;
const STRESS_PREKILL_SECS: &str = "630";
const STRESS_OUTER_SIGTERM_SECS: u64 = 660;
const STRESS_COMMAND: &str = r#"cargo run --locked --manifest-path "$GITHUB_WORKSPACE/controller/Cargo.toml" \
  -p xtask --bin xtask -- stress-run \
  --inventory "$RAW_DIR/inventory.json" \
  --config-file "$GITHUB_WORKSPACE/controller/.config/nextest.toml" \
  --filter "$FILTER" \
  --count "$COUNT" \
  --test-threads "$TEST_THREADS" \
  --flash "$FLASH" \
  --no-block "$NO_BLOCK" \
  2>&1 | tee "$RAW_DIR/nextest.log""#;

const VALIDATE_REVISION_SCRIPT: &str = r#"if [[ ! "$REQUESTED_REVISION" =~ ^[0-9a-fA-F]{40}$ ]]; then
  echo "revision must be a full 40-character commit SHA"
  exit 1
fi"#;

const CAPTURE_SUBJECT_SCRIPT: &str = r#"subject_sha="$(git rev-parse HEAD)"
if [[ "${subject_sha,,}" != "${REQUESTED_REVISION,,}" ]]; then
  echo "checked out $subject_sha instead of requested commit $REQUESTED_REVISION"
  exit 1
fi
printf 'sha=%s\n' "$subject_sha" >> "$GITHUB_OUTPUT""#;

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

const STRESS_AUTHORIZATION_SCRIPT: &str = r#"python3 - <<'PY'
import json
import os
import re
import sys

if os.environ.get("ENABLED") != "true":
    print("stress runs are disabled by KITHARA_STRESS_ENABLED")
    sys.exit(1)

if os.environ.get("IS_FORK") != "true":
    print("stress runs are restricted to repository forks")
    sys.exit(1)

actor = os.environ["ACTOR"]
owner = os.environ["OWNER"]
if actor != owner:
    print(f"stress may only be started by repository owner {owner!r}, got {actor!r}")
    sys.exit(1)
triggering_actor = os.environ["TRIGGERING_ACTOR"]
if triggering_actor != owner:
    print(
        "stress may only be started or re-run by repository owner "
        f"{owner!r}, got {triggering_actor!r}"
    )
    sys.exit(1)

raw_labels = os.environ.get("RUNNER_LABELS", "")
try:
    labels = json.loads(raw_labels)
except json.JSONDecodeError as error:
    print(f"KITHARA_STRESS_RUNNER_LABELS is not valid JSON: {error}")
    sys.exit(1)
if not isinstance(labels, list) or not labels or not all(
    isinstance(label, str) and label.strip() for label in labels
):
    print("KITHARA_STRESS_RUNNER_LABELS must be a non-empty JSON array of non-empty strings")
    sys.exit(1)
required_labels = {"self-hosted", "linux", "x64", "kithara"}
missing_labels = sorted(required_labels.difference(labels))
if missing_labels:
    print(
        "KITHARA_STRESS_RUNNER_LABELS is missing required labels: "
        + ", ".join(missing_labels)
    )
    sys.exit(1)

revision = os.environ["REVISION"]
if re.fullmatch(r"[0-9a-fA-F]{40}", revision) is None:
    print("revision must be a full 40-character commit SHA")
    sys.exit(1)

try:
    max_count = int(os.environ["MAX_COUNT"])
except ValueError:
    print("KITHARA_STRESS_MAX_COUNT must be an integer from 1 through 100")
    sys.exit(1)
if not 1 <= max_count <= 100:
    print("KITHARA_STRESS_MAX_COUNT must be an integer from 1 through 100")
    sys.exit(1)

try:
    count = int(os.environ["COUNT"])
except ValueError:
    print(f"count must be an integer from 1 through {max_count}")
    sys.exit(1)
if not 1 <= count <= max_count:
    print(f"count must be an integer from 1 through {max_count}")
    sys.exit(1)

if not os.environ["FILTER"].strip():
    print("filter must not be empty")
    sys.exit(1)

test_threads = os.environ["TEST_THREADS"]
if test_threads != "num-cpus":
    try:
        test_thread_count = int(test_threads)
    except ValueError:
        print("test_threads must be num-cpus or an integer from 1 through 256")
        sys.exit(1)
    if not 1 <= test_thread_count <= 256:
        print("test_threads must be num-cpus or an integer from 1 through 256")
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

fn workflow_concurrency(workflow: &Value) -> &Mapping {
    mapping_field(
        workflow.as_mapping().expect("workflow is a mapping"),
        "concurrency",
    )
    .as_mapping()
    .expect("workflow concurrency is a mapping")
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

fn step_position(job: &Mapping, name: &str) -> usize {
    mapping_field(job, "steps")
        .as_sequence()
        .expect("workflow steps are a sequence")
        .iter()
        .position(|step| {
            step.as_mapping()
                .and_then(|step| step.get("name"))
                .and_then(Value::as_str)
                == Some(name)
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
    let concurrency = workflow_concurrency(&workflow);
    assert_eq!(
        mapping_field(concurrency, "group").as_str(),
        Some(
            "${{ github.event.repository.fork && format('fork-linux-{0}', github.repository) || format('ci-{0}', github.ref) }}"
        )
    );
    assert_eq!(
        mapping_field(concurrency, "cancel-in-progress").as_str(),
        Some("${{ !github.event.repository.fork }}")
    );
    assert!(
        !concurrency.contains_key("queue"),
        "CI must preserve production cancel-in-progress semantics"
    );
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
    let concurrency = workflow_concurrency(&workflow);
    assert_eq!(
        mapping_field(concurrency, "group").as_str(),
        Some("fork-linux-${{ github.repository }}")
    );
    assert_eq!(
        mapping_field(concurrency, "cancel-in-progress").as_bool(),
        Some(false)
    );
    assert_eq!(mapping_field(concurrency, "queue").as_str(), Some("max"));
    let root = workflow.as_mapping().expect("workflow is a mapping");
    let permissions = mapping_field(root, "permissions")
        .as_mapping()
        .expect("permissions are a mapping");
    assert_eq!(permissions.len(), 1);
    assert_eq!(
        mapping_field(permissions, "contents").as_str(),
        Some("read")
    );
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
            BTreeSet::from([
                "count",
                "diagnostics",
                "filter",
                "flash",
                "no_block",
                "revision",
                "dump_thread_backtrace",
                "test_threads",
            ])
        );
        for name in ["revision", "filter", "count", "test_threads"] {
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
        let count_input = mapping_field(inputs, "count")
            .as_mapping()
            .expect("count input is a mapping");
        assert!(!count_input.contains_key("default"));
        assert_eq!(
            mapping_field(count_input, "description").as_str(),
            Some("repetitions; empty uses KITHARA_STRESS_COUNT")
        );
        assert_eq!(
            mapping_field(
                mapping_field(inputs, "test_threads")
                    .as_mapping()
                    .expect("test thread input is a mapping"),
                "default",
            )
            .as_str(),
            Some("num-cpus")
        );
        assert_eq!(
            mapping_field(
                mapping_field(inputs, "revision")
                    .as_mapping()
                    .expect("revision input is a mapping"),
                "description",
            )
            .as_str(),
            Some("full commit SHA to test; empty uses the triggering commit")
        );
        for (name, default) in [
            ("diagnostics", false),
            ("flash", true),
            ("no_block", false),
            ("dump_thread_backtrace", false),
        ] {
            let input = mapping_field(inputs, name)
                .as_mapping()
                .expect("diagnostic input definition is a mapping");
            assert_eq!(mapping_field(input, "type").as_str(), Some("boolean"));
            assert_eq!(mapping_field(input, "required").as_bool(), Some(false));
            assert_eq!(mapping_field(input, "default").as_bool(), Some(default));
        }
    }

    let jobs = workflow_jobs(&workflow);
    assert_eq!(
        workflow_job_names(jobs),
        BTreeSet::from([
            "authorize".to_owned(),
            "execute".to_owned(),
            "report".to_owned(),
        ])
    );

    let authorize = workflow_job(jobs, "authorize");
    assert_eq!(
        mapping_field(authorize, "runs-on").as_str(),
        Some("ubuntu-latest")
    );
    assert!(!authorize.contains_key("if"));
    let authorization = first_step(authorize);
    let authorization_env = mapping_field(authorization, "env")
        .as_mapping()
        .expect("stress authorization environment is a mapping");
    for (name, expected) in [
        ("ACTOR", "${{ github.actor }}"),
        (
            "COUNT",
            "${{ inputs.count || vars.KITHARA_STRESS_COUNT || '50' }}",
        ),
        ("ENABLED", "${{ vars.KITHARA_STRESS_ENABLED }}"),
        ("FILTER", "${{ inputs.filter || 'all()' }}"),
        ("IS_FORK", "${{ github.event.repository.fork }}"),
        ("MAX_COUNT", "${{ vars.KITHARA_STRESS_MAX_COUNT }}"),
        ("OWNER", "${{ github.repository_owner }}"),
        ("REVISION", "${{ inputs.revision || github.sha }}"),
        ("RUNNER_LABELS", "${{ vars.KITHARA_STRESS_RUNNER_LABELS }}"),
        ("TEST_THREADS", "${{ inputs.test_threads || 'num-cpus' }}"),
        ("TRIGGERING_ACTOR", "${{ github.triggering_actor }}"),
    ] {
        assert_eq!(
            mapping_field(authorization_env, name).as_str(),
            Some(expected)
        );
    }
    assert_eq!(
        mapping_field(authorization, "run")
            .as_str()
            .expect("stress authorization step is a script")
            .trim(),
        STRESS_AUTHORIZATION_SCRIPT
    );

    let execute = workflow_job(jobs, "execute");
    assert_eq!(job_needs(execute), BTreeSet::from(["authorize".to_owned()]));
    assert!(!execute.contains_key("if"));
    assert_eq!(
        mapping_field(execute, "runs-on").as_str(),
        Some("${{ fromJSON(vars.KITHARA_STRESS_RUNNER_LABELS) }}")
    );
    assert_eq!(
        mapping_field(execute, "timeout-minutes").as_u64(),
        Some(1380)
    );
    let outputs = mapping_field(execute, "outputs")
        .as_mapping()
        .expect("execute outputs are a mapping");
    assert_eq!(
        mapping_field(outputs, "subject-sha").as_str(),
        Some("${{ steps.subject-commit.outputs.sha }}")
    );

    let controller = named_step(execute, "Checkout controller");
    assert_eq!(mapping_field(controller, "uses").as_str(), Some(CHECKOUT));
    let controller_inputs = mapping_field(controller, "with")
        .as_mapping()
        .expect("checkout inputs are a mapping");
    assert_eq!(
        mapping_field(controller_inputs, "ref").as_str(),
        Some("${{ job.workflow_sha }}")
    );
    assert_eq!(
        mapping_field(controller_inputs, "repository").as_str(),
        Some("${{ job.workflow_repository }}")
    );
    assert_eq!(
        mapping_field(controller_inputs, "path").as_str(),
        Some("controller")
    );
    assert_eq!(
        mapping_field(controller_inputs, "persist-credentials").as_bool(),
        Some(false)
    );

    let validate = named_step(execute, "Validate immutable revision");
    let validate_env = mapping_field(validate, "env")
        .as_mapping()
        .expect("revision validation environment is a mapping");
    assert_eq!(
        mapping_field(validate_env, "REQUESTED_REVISION").as_str(),
        Some("${{ inputs.revision || github.sha }}")
    );
    assert_eq!(
        mapping_field(validate, "run")
            .as_str()
            .expect("revision validator is a script")
            .trim(),
        VALIDATE_REVISION_SCRIPT
    );

    let subject = named_step(execute, "Checkout subject");
    assert_eq!(mapping_field(subject, "uses").as_str(), Some(CHECKOUT));
    let subject_inputs = mapping_field(subject, "with")
        .as_mapping()
        .expect("checkout inputs are a mapping");
    assert_eq!(
        mapping_field(subject_inputs, "ref").as_str(),
        Some("${{ inputs.revision || github.sha }}")
    );
    assert_eq!(
        mapping_field(subject_inputs, "repository").as_str(),
        Some("${{ github.repository }}")
    );
    assert_eq!(
        mapping_field(subject_inputs, "path").as_str(),
        Some("subject")
    );
    assert_eq!(
        mapping_field(subject_inputs, "persist-credentials").as_bool(),
        Some(false)
    );

    let capture = named_step(execute, "Capture subject commit");
    assert_eq!(
        mapping_field(capture, "id").as_str(),
        Some("subject-commit")
    );
    assert_eq!(
        mapping_field(capture, "working-directory").as_str(),
        Some("subject")
    );
    let capture_env = mapping_field(capture, "env")
        .as_mapping()
        .expect("subject capture environment is a mapping");
    assert_eq!(
        mapping_field(capture_env, "REQUESTED_REVISION").as_str(),
        Some("${{ inputs.revision || github.sha }}")
    );
    assert_eq!(
        mapping_field(capture, "run")
            .as_str()
            .expect("subject capture is a script")
            .trim(),
        CAPTURE_SUBJECT_SCRIPT
    );
    assert!(
        step_position(execute, "Validate immutable revision")
            < step_position(execute, "Checkout subject")
    );
    assert!(
        step_position(execute, "Checkout subject")
            < step_position(execute, "Capture subject commit")
    );

    let prepare = named_step(execute, "Prepare raw stress evidence");
    assert_eq!(
        mapping_field(prepare, "working-directory").as_str(),
        Some("subject")
    );
    let prepare_env = mapping_field(prepare, "env")
        .as_mapping()
        .expect("raw evidence environment is a mapping");
    assert_eq!(
        mapping_field(prepare_env, "RAW_DIR").as_str(),
        Some(STRESS_RAW_DIR)
    );
    let prepare_script = mapping_field(prepare, "run")
        .as_str()
        .expect("raw evidence preparation is a script");
    for contract in [
        "rm -f target/nextest/stress/junit.xml",
        "mkdir -p \"$RAW_DIR/hang\"",
        ": > \"$RAW_DIR/nextest.log\"",
        ": > \"$RAW_DIR/pressure.jsonl\"",
    ] {
        assert!(
            prepare_script.contains(contract),
            "raw evidence preparation omits `{contract}`"
        );
    }
    assert!(
        step_position(execute, "Capture subject commit")
            < step_position(execute, "Prepare raw stress evidence")
    );
    assert!(
        step_position(execute, "Prepare raw stress evidence")
            < step_position(execute, "Repeat the selection")
    );

    let primary = named_step(execute, "Repeat the selection");
    assert_eq!(
        mapping_field(primary, "working-directory").as_str(),
        Some("subject")
    );
    let primary_env = mapping_field(primary, "env")
        .as_mapping()
        .expect("stress environment is a mapping");
    assert_eq!(
        primary_env
            .keys()
            .map(|name| name.as_str().expect("environment name is a string"))
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "CONTROLLER_SHA",
            "COUNT",
            "DIAGNOSTICS",
            "DIAGNOSTIC_RUST_LOG",
            "FILTER",
            "FLASH",
            "JOB_TIMEOUT_MINUTES",
            "KITHARA_HANG_DUMP_DIR",
            "KITHARA_HANG_PREKILL_SECS",
            "NEXTEST_FINAL_STATUS_LEVEL",
            "NEXTEST_SHOW_PROGRESS",
            "NEXTEST_STATUS_LEVEL",
            "NO_BLOCK",
            "NO_BLOCK_REQUESTED",
            "RAW_DIR",
            "REPRO_RUST_LOG",
            "RUST_BACKTRACE",
            "SUBJECT_SHA",
            "DUMP_THREAD_BACKTRACE",
            "DUMP_THREAD_BACKTRACE_REQUESTED",
            "TEST_THREADS",
        ])
    );
    for (name, expected) in [
        ("FILTER", "${{ inputs.filter || 'all()' }}"),
        (
            "COUNT",
            "${{ inputs.count || vars.KITHARA_STRESS_COUNT || '50' }}",
        ),
        ("TEST_THREADS", "${{ inputs.test_threads || 'num-cpus' }}"),
        ("FLASH", "${{ inputs.flash && 'true' || 'false' }}"),
        (
            "DIAGNOSTICS",
            "${{ inputs.diagnostics && 'true' || 'false' }}",
        ),
        (
            "NO_BLOCK_REQUESTED",
            "${{ inputs.no_block && 'true' || 'false' }}",
        ),
        (
            "NO_BLOCK",
            "${{ inputs.diagnostics && inputs.no_block && 'true' || 'false' }}",
        ),
        (
            "DUMP_THREAD_BACKTRACE_REQUESTED",
            "${{ inputs.dump_thread_backtrace && 'true' || 'false' }}",
        ),
        (
            "DUMP_THREAD_BACKTRACE",
            "${{ inputs.diagnostics && inputs.dump_thread_backtrace && 'true' || 'false' }}",
        ),
        ("RAW_DIR", STRESS_RAW_DIR),
        ("CONTROLLER_SHA", "${{ job.workflow_sha }}"),
        ("JOB_TIMEOUT_MINUTES", "1380"),
        ("SUBJECT_SHA", "${{ steps.subject-commit.outputs.sha }}"),
        ("RUST_BACKTRACE", "1"),
        ("REPRO_RUST_LOG", REPRO_RUST_LOG),
        ("DIAGNOSTIC_RUST_LOG", DIAGNOSTIC_RUST_LOG),
        (
            "KITHARA_HANG_DUMP_DIR",
            "${{ runner.temp }}/stress-raw-${{ github.run_id }}-${{ github.run_attempt }}/hang",
        ),
        ("KITHARA_HANG_PREKILL_SECS", STRESS_PREKILL_SECS),
        ("NEXTEST_STATUS_LEVEL", "fail"),
        ("NEXTEST_FINAL_STATUS_LEVEL", "fail"),
        ("NEXTEST_SHOW_PROGRESS", "counter"),
    ] {
        assert_eq!(
            mapping_field(primary_env, name).as_str(),
            Some(expected),
            "unexpected `{name}` value"
        );
    }

    let primary_script = mapping_field(primary, "run")
        .as_str()
        .expect("stress command is a script");
    assert!(
        primary_script.contains(STRESS_COMMAND),
        "stress workflow does not execute the controller command exactly"
    );
    for contract in [
        "if [[ \"$DIAGNOSTICS\" == 'true' ]]",
        "export RUST_LOG=\"$DIAGNOSTIC_RUST_LOG\"",
        "export KITHARA_FLASH_SYNC_TRACE=1",
        "if [[ \"$DUMP_THREAD_BACKTRACE\" == 'true' ]]",
        "export KITHARA_FLASH_SYNC_BT=1",
        "if [[ \"$NO_BLOCK\" == 'true' ]]",
        ": > \"$RAW_DIR/no-block.log\"",
        "export KITHARA_NO_BLOCK=census",
        "export KITHARA_NO_BLOCK_BUDGET_MS=100",
        "export KITHARA_NO_BLOCK_LOG=\"$RAW_DIR/no-block.log\"",
        "export RUST_LOG=\"$REPRO_RUST_LOG\"",
        "unset KITHARA_FLASH_SYNC_TRACE KITHARA_FLASH_SYNC_BT",
        "unset KITHARA_NO_BLOCK KITHARA_NO_BLOCK_BUDGET_MS KITHARA_NO_BLOCK_LOG",
    ] {
        assert!(
            primary_script.contains(contract),
            "stress mode separation omits `{contract}`"
        );
    }
    for contract in [
        "write_manifest()",
        "MANIFEST_ENDED_AT=\"$1\" MANIFEST_EXIT_CODE=\"$2\" python3",
        "\"mode\": \"diagnostic\" if enabled(\"DIAGNOSTICS\") else \"reproduction\"",
        "\"config\": {",
        "\"pressure_schema\": \"kithara.pressure.v1\"",
        "\"workflow_job_timeout_minutes\": int(os.environ[\"JOB_TIMEOUT_MINUTES\"])",
        "\"controller\": {\"sha\": os.environ[\"CONTROLLER_SHA\"]}",
        "\"subject\": {\"sha\": os.environ[\"SUBJECT_SHA\"]}",
        "\"run\": {",
        "\"selection\": {",
        "\"test_threads\": os.environ[\"TEST_THREADS\"]",
        "\"features\": {",
        "\"logging\": {",
        "\"hang_prekill_secs\": optional(\"KITHARA_HANG_PREKILL_SECS\")",
        "\"no_block_log\": \"raw/no-block.log\" if optional(\"KITHARA_NO_BLOCK_LOG\") else None",
        "\"hang_dump_dir\": \"raw/hang\"",
        "\"nextest_status_level\": os.environ[\"NEXTEST_STATUS_LEVEL\"]",
        "\"nextest_final_status_level\": os.environ[\"NEXTEST_FINAL_STATUS_LEVEL\"]",
        "\"nextest_show_progress\": os.environ[\"NEXTEST_SHOW_PROGRESS\"]",
        "\"timing\": {",
        "\"started_at\": os.environ[\"STARTED_AT\"]",
        "\"ended_at\": optional(\"MANIFEST_ENDED_AT\")",
        "\"pressure\": {",
        "\"sampler_healthy\": optional(\"PRESSURE_SAMPLER_HEALTHY\")",
        "\"system\": {",
        "\"kernel\": os.environ[\"KERNEL\"]",
        "\"scope\": os.environ[\"CGROUP_SCOPE\"]",
        "\"path\": os.environ[\"CGROUP_DIR\"] if os.environ[\"CGROUP_SCOPE\"] != \"unavailable\" else None",
        "\"cpuset\": {",
        "\"limits\": {",
        "Path(os.environ[\"RAW_DIR\"]) / \"manifest.json\"",
        "write_manifest '' ''",
        "write_manifest \"$(date --iso-8601=ns)\" \"$status\"",
    ] {
        assert!(
            primary_script.contains(contract),
            "stress manifest omits `{contract}`"
        );
    }
    assert!(!primary_script.contains("dict(os.environ)"));
    for contract in [
        "set -o pipefail",
        "sample_pressure()",
        "exec python3 - \"$RAW_DIR/pressure.jsonl\" \"$CGROUP_DIR\" \"$CGROUP_SCOPE\"",
        "import signal",
        "signal.signal(signal.SIGTERM, stop)",
        "while running:",
        "stream.flush()",
        "$CGROUP_DIR/cgroup.controllers",
        "CGROUP_SCOPE=current_process_cgroup",
        "CGROUP_SCOPE=unavailable",
        "date --iso-8601=ns",
        "/proc/loadavg",
        "/proc/pressure/cpu",
        "/proc/pressure/memory",
        "/proc/pressure/io",
        "/proc/meminfo",
        "/proc/stat",
        "cpu.stat",
        "cpu.pressure",
        "memory.current",
        "memory.peak",
        "memory.events",
        "memory.pressure",
        "io.stat",
        "io.pressure",
        "pids.current",
        "pids.events",
        "cpuset.cpus.effective",
        "\"schema\": \"kithara.pressure.v1\"",
        "\"marker\": \"start\" if first else \"sample\"",
        "\"timestamp_ms\": time.time_ns() // 1_000_000",
        "\"load1\": load_one(loadavg)",
        "\"metrics\": metrics",
        "\"proc.pressure.cpu\"",
        "\"cgroup.cpu.stat\"",
        "\"proc_pressure\": \"host\"",
        "\"cgroup_v2\": cgroup_scope",
        "time.sleep(1)",
        "trap finish EXIT",
        "trap 'exit 130' INT",
        "trap 'exit 143' TERM",
        "sample_pressure &",
        "pressure_ready=false",
        "grep -q '\"marker\":\"start\"' \"$RAW_DIR/pressure.jsonl\"",
        "export PRESSURE_SAMPLER_HEALTHY=true",
        "write_pressure_end()",
        "\"marker\": \"end\"",
        "\"sampler_healthy\": os.environ[\"PRESSURE_SAMPLER_HEALTHY\"] == \"true\"",
        "PRESSURE_EXIT_CODE=\"$1\" python3",
        "write_pressure_end \"$status\"",
        "if [[ \"$PRESSURE_SAMPLER_HEALTHY\" != 'true' && \"$status\" -eq 0 ]]",
        "if ! kill -0 \"$pressure_pid\" 2>/dev/null",
        "if ! kill \"$pressure_pid\" 2>/dev/null",
        "pressure_status=0",
        "wait \"$pressure_pid\" 2>/dev/null || pressure_status=$?",
        "if [[ \"$pressure_status\" -ne 0 ]]",
        "kill \"$pressure_pid\"",
        "wait \"$pressure_pid\"",
        "primary_status=${PIPESTATUS[0]}",
        "if ! kill -0 \"$pressure_pid\" 2>/dev/null",
        "export PRESSURE_SAMPLER_HEALTHY=false",
        "exit \"$primary_status\"",
    ] {
        assert!(
            primary_script.contains(contract),
            "stress pressure capture omits `{contract}`"
        );
    }
    for forbidden in ["--retries", "--fail-fast", "--no-fail-fast", "--max-fail"] {
        assert!(
            !primary_script.contains(forbidden),
            "stress command overrides `{forbidden}`"
        );
    }

    let stage = named_step(execute, "Stage the raw stress evidence");
    assert_always(stage);
    let stage_env = mapping_field(stage, "env")
        .as_mapping()
        .expect("raw evidence staging environment is a mapping");
    assert_eq!(
        mapping_field(stage_env, "RAW_DIR").as_str(),
        Some(STRESS_RAW_DIR)
    );
    assert_eq!(
        mapping_field(stage_env, "SUBJECT_SHA").as_str(),
        Some("${{ steps.subject-commit.outputs.sha || 'unresolved' }}")
    );
    let stage_script = mapping_field(stage, "run")
        .as_str()
        .expect("raw evidence staging is a script");
    for contract in [
        "mkdir -p \"$RAW_DIR/hang\"",
        "[[ -f \"$RAW_DIR/nextest.log\" ]] || : > \"$RAW_DIR/nextest.log\"",
        "[[ -f \"$RAW_DIR/pressure.jsonl\" ]] || : > \"$RAW_DIR/pressure.jsonl\"",
        "subject-sha.txt",
        "target/nextest/stress/junit.xml",
        "$RAW_DIR/junit.xml",
        "$SUBJECT_SHA",
    ] {
        assert!(
            stage_script.contains(contract),
            "raw evidence staging omits `{contract}`"
        );
    }
    assert_uploads(
        named_step(execute, "Upload the raw stress evidence"),
        &[STRESS_RAW_DIR],
    );
    assert_eq!(
        mapping_field(
            mapping_field(
                named_step(execute, "Upload the raw stress evidence"),
                "with"
            )
            .as_mapping()
            .expect("raw upload inputs are a mapping"),
            "retention-days",
        )
        .as_u64(),
        Some(14)
    );

    let report_job = workflow_job(jobs, "report");
    assert_eq!(
        mapping_field(report_job, "runs-on").as_str(),
        Some("ubuntu-latest")
    );
    assert_eq!(
        job_needs(report_job),
        BTreeSet::from(["execute".to_owned()])
    );
    assert_eq!(
        mapping_field(report_job, "if").as_str(),
        Some("${{ always() && needs.execute.result != 'skipped' }}")
    );

    let report_controller = named_step(report_job, "Checkout controller");
    assert_eq!(
        mapping_field(report_controller, "uses").as_str(),
        Some(CHECKOUT)
    );
    let report_controller_inputs = mapping_field(report_controller, "with")
        .as_mapping()
        .expect("report checkout inputs are a mapping");
    assert_eq!(
        mapping_field(report_controller_inputs, "ref").as_str(),
        Some("${{ job.workflow_sha }}")
    );
    assert_eq!(
        mapping_field(report_controller_inputs, "repository").as_str(),
        Some("${{ job.workflow_repository }}")
    );
    assert_eq!(
        mapping_field(report_controller_inputs, "persist-credentials").as_bool(),
        Some(false)
    );

    let download = named_step(report_job, "Download the raw stress evidence");
    assert_eq!(
        mapping_field(download, "uses").as_str(),
        Some(DOWNLOAD_ARTIFACT)
    );
    let download_inputs = mapping_field(download, "with")
        .as_mapping()
        .expect("artifact download inputs are a mapping");
    assert_eq!(
        mapping_field(download_inputs, "name").as_str(),
        Some("stress-raw-${{ github.run_id }}-${{ github.run_attempt }}")
    );
    assert_eq!(mapping_field(download_inputs, "path").as_str(), Some("raw"));

    let provenance = named_step(report_job, "Validate stress provenance");
    assert_always(provenance);
    let provenance_env = mapping_field(provenance, "env")
        .as_mapping()
        .expect("provenance environment is a mapping");
    for (name, expected) in [
        ("CONTROLLER_SHA", "${{ job.workflow_sha }}"),
        (
            "COUNT",
            "${{ inputs.count || vars.KITHARA_STRESS_COUNT || '50' }}",
        ),
        ("EXECUTE_RESULT", "${{ needs.execute.result }}"),
        ("FILTER", "${{ inputs.filter || 'all()' }}"),
        ("JOB_TIMEOUT_MINUTES", "1380"),
        (
            "MODE",
            "${{ inputs.diagnostics && 'diagnostic' || 'reproduction' }}",
        ),
        (
            "SUBJECT_SHA",
            "${{ needs.execute.outputs.subject-sha || 'unresolved' }}",
        ),
        ("TEST_THREADS", "${{ inputs.test_threads || 'num-cpus' }}"),
    ] {
        assert_eq!(mapping_field(provenance_env, name).as_str(), Some(expected));
    }
    let provenance_script = mapping_field(provenance, "run")
        .as_str()
        .expect("provenance validator is a script");
    for contract in [
        "manifest_path = raw / \"manifest.json\"",
        "if not manifest_path.is_file() or manifest_path.stat().st_size == 0",
        "(\"controller\", \"sha\"): os.environ[\"CONTROLLER_SHA\"]",
        "(\"subject\", \"sha\"): os.environ[\"SUBJECT_SHA\"]",
        "(\"selection\", \"filter\"): os.environ[\"FILTER\"]",
        "(\"selection\", \"count\"): os.environ[\"COUNT\"]",
        "(\"selection\", \"test_threads\"): os.environ[\"TEST_THREADS\"]",
        "(\"config\", \"workflow_job_timeout_minutes\"): int(os.environ[\"JOB_TIMEOUT_MINUTES\"])",
        "if manifest.get(\"mode\") != os.environ[\"MODE\"]",
        "timing.ended_at is not finalized",
        "timing.exit_code={exit_code!r} is not finalized",
        "result == \"success\" and exit_code != 0",
        "result in {\"failure\", \"cancelled\"} and exit_code == 0",
        "pressure_path = raw / \"pressure.jsonl\"",
        "with pressure_path.open(\"r\", encoding=\"utf-8\") as stream",
        "pressure_malformed += 1",
        "pressure_count < 2",
        "pressure_first.get(\"marker\") != \"start\"",
        "pressure_last.get(\"marker\") != \"end\"",
        "pressure_last.get(\"sampler_healthy\") is not True",
        "pressure.get(\"sampler_healthy\") != \"true\"",
        "sys.exit(1)",
    ] {
        assert!(
            provenance_script.contains(contract),
            "stress provenance validation omits `{contract}`"
        );
    }
    assert!(
        step_position(report_job, "Download the raw stress evidence")
            < step_position(report_job, "Validate stress provenance")
    );
    assert!(
        step_position(report_job, "Validate stress provenance")
            < step_position(report_job, "Summarize the stress evidence")
    );

    let install = named_step(report_job, "Install just");
    assert_always(install);
    assert_eq!(
        mapping_field(install, "uses").as_str(),
        Some(INSTALL_ACTION)
    );
    let install_inputs = mapping_field(install, "with")
        .as_mapping()
        .expect("tool installation inputs are a mapping");
    assert_eq!(mapping_field(install_inputs, "tool").as_str(), Some("just"));

    let report = named_step(report_job, "Summarize the stress evidence");
    assert_always(report);
    let report_env = mapping_field(report, "env")
        .as_mapping()
        .expect("stress report environment is a mapping");
    assert_eq!(
        report_env
            .keys()
            .map(|name| name.as_str().expect("report environment name is a string"))
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["COUNT", "FILTER", "MODE", "SUBJECT_SHA", "TEST_THREADS"])
    );
    assert_eq!(
        mapping_field(report_env, "SUBJECT_SHA").as_str(),
        Some("${{ needs.execute.outputs.subject-sha || 'unresolved' }}")
    );
    assert_eq!(
        mapping_field(report_env, "FILTER").as_str(),
        Some("${{ inputs.filter || 'all()' }}")
    );
    assert_eq!(
        mapping_field(report_env, "COUNT").as_str(),
        Some("${{ inputs.count || vars.KITHARA_STRESS_COUNT || '50' }}")
    );
    assert_eq!(
        mapping_field(report_env, "MODE").as_str(),
        Some("${{ inputs.diagnostics && 'diagnostic' || 'reproduction' }}")
    );
    assert_eq!(
        mapping_field(report_env, "TEST_THREADS").as_str(),
        Some("${{ inputs.test_threads || 'num-cpus' }}")
    );
    let report_script = mapping_field(report, "run")
        .as_str()
        .expect("stress report is a script");
    for contract in [
        "stress-report\n  --allow-missing\n  --inventory \"$GITHUB_WORKSPACE/raw/inventory.json\"\n  --expected-count \"$COUNT\"\n  --junit \"$GITHUB_WORKSPACE/raw/junit.xml\"\n  --pressure-log \"$GITHUB_WORKSPACE/raw/pressure.jsonl\"\n  --output \"$report_path\"",
        "if [[ -d \"$GITHUB_WORKSPACE/raw/hang\" ]]",
        "report_args+=(--hang-dir \"$GITHUB_WORKSPACE/raw/hang\")",
        "if [[ -f \"$GITHUB_WORKSPACE/raw/no-block.log\" ]]",
        "report_args+=(--no-block-log \"$GITHUB_WORKSPACE/raw/no-block.log\")",
        "just tooling xtask \"${report_args[@]}\" || report_status=$?",
        "report_path=\"$GITHUB_WORKSPACE/target/stress-report.md\"",
        "if [[ ! -s \"$report_path\" ]]",
        "if [[ \"$report_status\" -eq 0 ]]",
        "report_status=1",
        "The reporter produced no non-empty report",
        "$GITHUB_STEP_SUMMARY",
        "$SUBJECT_SHA",
        "$FILTER",
        "$MODE",
        "$TEST_THREADS",
        "|| report_status=$?",
        "exit \"$report_status\"",
    ] {
        assert!(
            report_script.contains(contract),
            "stress report omits `{contract}`"
        );
    }

    assert_uploads(
        named_step(report_job, "Upload the stress evidence"),
        &["raw", "target/stress-report.md"],
    );
    assert_eq!(
        mapping_field(
            mapping_field(named_step(report_job, "Upload the stress evidence"), "with")
                .as_mapping()
                .expect("report upload inputs are a mapping"),
            "retention-days",
        )
        .as_u64(),
        Some(14)
    );
}

#[test]
fn scheduled_stress_respects_the_repository_switch_and_runner_pool() {
    let workflow = github_workflow("schedule.yml");
    let stress = workflow_job(workflow_jobs(&workflow), "stress");
    assert_eq!(
        mapping_field(stress, "uses").as_str(),
        Some("./.github/workflows/stress.yml")
    );
    let condition = mapping_field(stress, "if")
        .as_str()
        .expect("scheduled stress condition is a string");
    for contract in [
        "vars.KITHARA_STRESS_ENABLED == 'true'",
        "vars.KITHARA_STRESS_RUNNER_LABELS != ''",
        "github.event.schedule == '0 3 * * *'",
    ] {
        assert!(
            condition.contains(contract),
            "scheduled stress omits `{contract}`"
        );
    }
}

#[test]
fn stress_profile_records_failures_with_a_dump_aware_outer_backstop() {
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
    assert_eq!(stress["fail-fast"].as_bool(), Some(false));
    let timeout = stress["slow-timeout"]
        .as_table()
        .expect("stress timeout is a table");
    let period = timeout["period"]
        .as_str()
        .and_then(|period| period.strip_suffix('s'))
        .and_then(|seconds| seconds.parse::<u64>().ok())
        .expect("stress timeout period is seconds");
    let terminate_after = timeout["terminate-after"]
        .as_integer()
        .and_then(|count| u64::try_from(count).ok())
        .expect("stress termination count is positive");
    let outer = period * terminate_after;
    let prekill = STRESS_PREKILL_SECS
        .parse::<u64>()
        .expect("stress pre-kill deadline is seconds");
    assert_eq!(outer, STRESS_OUTER_SIGTERM_SECS);
    assert!(prekill >= STRESS_MAX_DECLARED_TIMEOUT_SECS + 30);
    assert!(outer >= prekill + 30);

    let junit = stress["junit"].as_table().expect("stress JUnit is a table");
    assert_eq!(junit["path"].as_str(), Some("junit.xml"));
    assert_eq!(junit["store-failure-output"].as_bool(), Some(true));
}

#[test]
fn stress_backstop_covers_every_kithara_test_timeout() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a workspace root");
    let mut observed_max = 0_u64;
    let mut excessive = Vec::new();
    let mut unsupported = Vec::new();
    let mut observed = 0_usize;
    for tree in ["crates", "tests"] {
        let pattern = root.join(tree).join("**/*.rs");
        let pattern = pattern.to_str().expect("source glob path is UTF-8");
        for path in glob::glob(pattern).expect("source glob is valid") {
            let path = path.expect("source path is readable");
            let source = fs::read_to_string(&path).expect("Rust source is readable");
            let file = match syn::parse_file(&source) {
                Ok(file) => file,
                Err(error) => {
                    unsupported.push(format!(
                        "{}: cannot parse Rust source: {error}",
                        path.display()
                    ));
                    continue;
                }
            };
            let mut constants = TimeoutConstants::default();
            constants.visit_file(&file);
            let tokens = match source.parse::<TokenStream>() {
                Ok(tokens) => tokens,
                Err(error) => {
                    unsupported.push(format!(
                        "{}: cannot tokenize Rust source: {error}",
                        path.display()
                    ));
                    continue;
                }
            };
            let mut expressions = Vec::new();
            collect_timeout_attributes(&tokens, &path, &mut expressions, &mut unsupported);
            for expression in expressions {
                observed = observed.saturating_add(1);
                let Some(seconds) = timeout_seconds(&expression, &constants.values) else {
                    unsupported.push(format!(
                        "{}: unsupported kithara test timeout expression",
                        path.display()
                    ));
                    continue;
                };
                observed_max = observed_max.max(seconds);
                if seconds > STRESS_MAX_DECLARED_TIMEOUT_SECS {
                    excessive.push(format!("{}: {seconds}s", path.display()));
                }
            }
        }
    }

    assert!(observed > 0, "no kithara test timeout attributes found");
    assert!(
        unsupported.is_empty(),
        "kithara test timeouts escaped the closed source audit: {unsupported:?}"
    );
    assert!(
        excessive.is_empty(),
        "declared test timeouts exceed the stress evidence model: {excessive:?}"
    );
    assert_eq!(observed_max, STRESS_MAX_DECLARED_TIMEOUT_SECS);
}

#[derive(Default)]
struct TimeoutConstants {
    values: BTreeMap<String, u64>,
}

impl<'ast> Visit<'ast> for TimeoutConstants {
    fn visit_item_const(&mut self, item: &'ast ItemConst) {
        if let Some(value) = integer_seconds(&item.expr, &self.values) {
            self.values
                .entry(item.ident.to_string())
                .and_modify(|current| *current = (*current).max(value))
                .or_insert(value);
        }
    }
}

fn collect_timeout_attributes(
    stream: &TokenStream,
    path: &Path,
    expressions: &mut Vec<Expr>,
    problems: &mut Vec<String>,
) {
    let tokens = stream.clone().into_iter().collect::<Vec<_>>();
    let mut index = 0;
    while index < tokens.len() {
        if matches!(&tokens[index], TokenTree::Punct(punct) if punct.as_char() == '#') {
            let mut group_index = index + 1;
            if matches!(tokens.get(group_index), Some(TokenTree::Punct(punct)) if punct.as_char() == '!')
            {
                group_index += 1;
            }
            if let Some(TokenTree::Group(group)) = tokens.get(group_index)
                && group.delimiter() == Delimiter::Bracket
            {
                collect_timeout_meta(&group.stream(), path, expressions, problems);
                collect_timeout_attributes(&group.stream(), path, expressions, problems);
                index = group_index + 1;
                continue;
            }
        }
        if let TokenTree::Group(group) = &tokens[index] {
            collect_timeout_attributes(&group.stream(), path, expressions, problems);
        }
        index += 1;
    }
}

fn collect_timeout_meta(
    tokens: &TokenStream,
    path: &Path,
    expressions: &mut Vec<Expr>,
    problems: &mut Vec<String>,
) {
    let meta = match syn::parse2::<Meta>(tokens.clone()) {
        Ok(meta) => meta,
        Err(error) => {
            if starts_with_ident(tokens, "kithara") || starts_with_ident(tokens, "cfg_attr") {
                problems.push(format!(
                    "{}: cannot parse timeout-bearing attribute: {error}",
                    path.display()
                ));
            }
            return;
        }
    };
    collect_parsed_timeout_meta(&meta, path, expressions, problems);
}

fn collect_parsed_timeout_meta(
    meta: &Meta,
    path: &Path,
    expressions: &mut Vec<Expr>,
    problems: &mut Vec<String>,
) {
    if is_kithara_test(meta.path()) {
        let Meta::List(list) = meta else {
            return;
        };
        let arguments =
            match Punctuated::<Expr, Token![,]>::parse_terminated.parse2(list.tokens.clone()) {
                Ok(arguments) => arguments,
                Err(error) => {
                    problems.push(format!(
                        "{}: cannot parse kithara test attribute: {error}",
                        path.display()
                    ));
                    return;
                }
            };
        for argument in arguments {
            let Expr::Call(call) = argument else {
                continue;
            };
            if call_name(&call).as_deref() != Some("timeout") {
                continue;
            }
            if call.args.len() != 1 {
                problems.push(format!(
                    "{}: timeout must contain exactly one expression",
                    path.display()
                ));
                continue;
            }
            expressions.extend(call.args.first().cloned());
        }
        return;
    }

    if meta.path().is_ident("cfg_attr")
        && let Meta::List(list) = meta
    {
        let nested =
            match Punctuated::<Meta, Token![,]>::parse_terminated.parse2(list.tokens.clone()) {
                Ok(nested) => nested,
                Err(error) => {
                    problems.push(format!(
                        "{}: cannot parse cfg_attr while auditing timeouts: {error}",
                        path.display()
                    ));
                    return;
                }
            };
        for nested_meta in nested.iter().skip(1) {
            collect_parsed_timeout_meta(nested_meta, path, expressions, problems);
        }
    }
}

fn starts_with_ident(tokens: &TokenStream, expected: &str) -> bool {
    matches!(tokens.clone().into_iter().next(), Some(TokenTree::Ident(ident)) if ident == expected)
}

fn is_kithara_test(path: &syn::Path) -> bool {
    let mut segments = path.segments.iter();
    segments
        .next()
        .is_some_and(|segment| segment.ident == "kithara")
        && segments
            .next()
            .is_some_and(|segment| segment.ident == "test")
        && segments.next().is_none()
}

#[test]
fn timeout_source_audit_reads_macro_rules_and_cfg_attr_tokens() {
    let source = r##"
        const TEXT: &str = "#[kithara::test(timeout(Duration::from_secs(999)))]";
        macro_rules! generated_test {
            () => {
                #[cfg_attr(unix, kithara::test(tokio, timeout(Duration::from_secs(5))))]
                async fn generated() {}
            };
        }
    "##;
    let tokens = source.parse::<TokenStream>().expect("fixture tokenizes");
    let mut expressions = Vec::new();
    let mut problems = Vec::new();
    collect_timeout_attributes(
        &tokens,
        Path::new("macro-fixture.rs"),
        &mut expressions,
        &mut problems,
    );

    assert!(
        problems.is_empty(),
        "unexpected audit problems: {problems:?}"
    );
    assert_eq!(expressions.len(), 1);
    assert_eq!(timeout_seconds(&expressions[0], &BTreeMap::new()), Some(5));
}

fn timeout_seconds(expression: &Expr, constants: &BTreeMap<String, u64>) -> Option<u64> {
    match expression {
        Expr::Call(call)
            if call_name(call).as_deref() == Some("from_secs") && call.args.len() == 1 =>
        {
            let segments = call_path(call)?;
            (segments.iter().rev().nth(1).map(String::as_str) == Some("Duration"))
                .then(|| call.args.first())
                .flatten()
                .and_then(|argument| integer_seconds(argument, constants))
        }
        Expr::Call(call)
            if call_name(call).as_deref() == Some("browser_timeout") && call.args.len() == 2 =>
        {
            call.args
                .iter()
                .map(|argument| integer_seconds(argument, constants))
                .collect::<Option<Vec<_>>>()
                .and_then(|values| values.into_iter().max())
        }
        Expr::If(branch) => {
            let then_value = block_value(&branch.then_branch)
                .and_then(|value| timeout_seconds(value, constants))?;
            let else_value = branch
                .else_branch
                .as_ref()
                .and_then(|(_, value)| timeout_seconds(value, constants))?;
            Some(then_value.max(else_value))
        }
        Expr::Block(block) => {
            block_value(&block.block).and_then(|value| timeout_seconds(value, constants))
        }
        Expr::Group(group) => timeout_seconds(&group.expr, constants),
        Expr::Paren(paren) => timeout_seconds(&paren.expr, constants),
        _ => None,
    }
}

fn integer_seconds(expression: &Expr, constants: &BTreeMap<String, u64>) -> Option<u64> {
    match expression {
        Expr::Lit(literal) => match &literal.lit {
            syn::Lit::Int(value) => value.base10_parse().ok(),
            _ => None,
        },
        Expr::Path(path) if path.qself.is_none() && path.path.segments.len() == 1 => constants
            .get(&path.path.segments.first()?.ident.to_string())
            .copied(),
        Expr::Binary(binary) if matches!(binary.op, BinOp::Add(_)) => {
            integer_seconds(&binary.left, constants)?
                .checked_add(integer_seconds(&binary.right, constants)?)
        }
        Expr::Group(group) => integer_seconds(&group.expr, constants),
        Expr::Paren(paren) => integer_seconds(&paren.expr, constants),
        _ => None,
    }
}

fn call_name(call: &syn::ExprCall) -> Option<String> {
    call_path(call).and_then(|segments| segments.last().cloned())
}

fn call_path(call: &syn::ExprCall) -> Option<Vec<String>> {
    let Expr::Path(path) = call.func.as_ref() else {
        return None;
    };
    Some(
        path.path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect(),
    )
}

fn block_value(block: &syn::Block) -> Option<&Expr> {
    match block.stmts.last()? {
        Stmt::Expr(expression, None) => Some(expression),
        _ => None,
    }
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
