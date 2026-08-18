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
const STRESS_RAW_DIR: &str = "${{ runner.temp }}/kithara-stress/raw";
const STRESS_EXECUTE_COMMAND: &str = r#"args=(
  --subject-root "$GITHUB_WORKSPACE/subject"
  --output "$RUNNER_TEMP/kithara-stress/raw"
  --expected-controller-sha "$CONTROLLER_SHA"
  --expected-subject-sha "$SUBJECT_SHA"
)
[[ -z "$FILTER" ]] || args+=(--filter "$FILTER")
[[ -z "$COUNT" ]] || args+=(--count "$COUNT")
[[ -z "$MODE" ]] || args+=(--mode "$MODE")
just ci stress "${args[@]}""#;
const STRESS_REPORT_COMMAND: &str = r#"args=(
  --raw "$GITHUB_WORKSPACE/raw"
  --output "$GITHUB_WORKSPACE/target/stress-report.md"
  --expected-controller-sha "$CONTROLLER_SHA"
  --expected-subject-sha "$SUBJECT_SHA"
  --execute-result "$EXECUTE_RESULT"
)
[[ -z "$FILTER" ]] || args+=(--filter "$FILTER")
[[ -z "$COUNT" ]] || args+=(--count "$COUNT")
[[ -z "$MODE" ]] || args+=(--mode "$MODE")
just ci stress-report "${args[@]}""#;

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
    fs::read_to_string(workflows_dir().join(name)).expect("workflow is readable")
}

fn workflows_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a workspace root")
        .join(".github/workflows")
}

fn workflow_file_names() -> BTreeSet<String> {
    fs::read_dir(workflows_dir())
        .expect("workflow directory is readable")
        .map(|entry| {
            entry
                .expect("workflow entry is readable")
                .file_name()
                .to_str()
                .expect("workflow file name is UTF-8")
                .to_owned()
        })
        .filter(|name| name.ends_with(".yml"))
        .collect()
}

fn configured_stress_prekill_secs(root: &Path) -> u64 {
    let config: toml::Value = toml::from_str(
        &fs::read_to_string(root.join(".config/xtask.toml")).expect("xtask config is readable"),
    )
    .expect("xtask config is valid TOML");
    let modes = config["stress"]["modes"]
        .as_table()
        .expect("stress modes are configured");
    let values = modes
        .values()
        .filter_map(|mode| mode.get("set_env"))
        .filter_map(toml::Value::as_table)
        .filter_map(|environment| environment.get("KITHARA_HANG_PREKILL_SECS"))
        .filter_map(toml::Value::as_str)
        .map(|value| value.parse::<u64>().expect("pre-kill value is seconds"))
        .collect::<BTreeSet<_>>();
    assert_eq!(values.len(), 1, "stress modes disagree on pre-kill timing");
    values
        .into_iter()
        .next()
        .expect("pre-kill timing is configured")
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

fn job_step_names(job: &Mapping) -> BTreeSet<String> {
    mapping_field(job, "steps")
        .as_sequence()
        .expect("workflow steps are a sequence")
        .iter()
        .map(|step| {
            mapping_field(
                step.as_mapping().expect("workflow step is a mapping"),
                "name",
            )
            .as_str()
            .expect("workflow step name is a string")
            .to_owned()
        })
        .collect()
}

fn assert_checkout(step: &Mapping, path: &str, repository: &str, revision: &str) {
    assert_eq!(mapping_field(step, "uses").as_str(), Some(CHECKOUT));
    let inputs = mapping_field(step, "with")
        .as_mapping()
        .expect("checkout inputs are a mapping");
    assert_eq!(mapping_field(inputs, "path").as_str(), Some(path));
    assert_eq!(
        mapping_field(inputs, "repository").as_str(),
        Some(repository)
    );
    assert_eq!(mapping_field(inputs, "ref").as_str(), Some(revision));
    assert_eq!(
        mapping_field(inputs, "persist-credentials").as_bool(),
        Some(false)
    );
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

fn concurrency_prefixes(owner: &Mapping) -> BTreeSet<String> {
    let Some(concurrency) = owner.get("concurrency") else {
        return BTreeSet::new();
    };
    let group = match concurrency {
        Value::String(group) => group.as_str(),
        Value::Mapping(mapping) => mapping_field(mapping, "group")
            .as_str()
            .expect("concurrency group is a string"),
        _ => panic!("concurrency is a string or a mapping"),
    };
    let formatted: BTreeSet<String> = group
        .match_indices("format('")
        .map(|(index, marker)| {
            let literal = group[index + marker.len()..]
                .split('\'')
                .next()
                .expect("format template is quoted");
            group_prefix(literal)
        })
        .collect();
    if formatted.is_empty() {
        BTreeSet::from([group_prefix(group)])
    } else {
        formatted
    }
}

#[test]
fn a_queue_is_never_declared_beside_a_cancellation_that_can_fire() {
    let mut conflicts = Vec::new();
    for name in workflow_file_names() {
        let workflow = github_workflow(&name);
        let mut owners = vec![("".to_owned(), workflow.as_mapping().cloned())];
        for (job_name, job) in workflow_jobs(&workflow) {
            let job_name = job_name.as_str().expect("workflow job name is a string");
            owners.push((format!(" job `{job_name}`"), job.as_mapping().cloned()));
        }
        for (where_, owner) in owners {
            let Some(Value::Mapping(concurrency)) =
                owner.as_ref().and_then(|owner| owner.get("concurrency"))
            else {
                continue;
            };
            if concurrency.get("queue").and_then(Value::as_str) != Some("max") {
                continue;
            }
            // GitHub rejects `queue: max` beside `cancel-in-progress: true`, and
            // an expression that reads false on a fork still reads true upstream.
            if concurrency
                .get("cancel-in-progress")
                .and_then(Value::as_bool)
                != Some(false)
            {
                conflicts.push(format!("{name}{where_}"));
            }
        }
    }

    assert!(
        conflicts.is_empty(),
        "`queue: max` needs `cancel-in-progress: false`, not an expression that can read true: {conflicts:?}"
    );
}

// What a group expression can evaluate to, down to the part no branch varies:
// `fork-linux-{0}` and `fork-linux-${{ github.repository }}` name one queue.
fn group_prefix(group: &str) -> String {
    group
        .split(['{', '$'])
        .next()
        .unwrap_or(group)
        .trim()
        .to_owned()
}

#[test]
fn a_called_workflow_never_waits_on_the_group_its_caller_holds() {
    let mut deadlocks = Vec::new();
    for name in workflow_file_names() {
        let workflow = github_workflow(&name);
        let held = concurrency_prefixes(workflow.as_mapping().expect("workflow is a mapping"));
        for (job_name, job) in workflow_jobs(&workflow) {
            let job = job.as_mapping().expect("workflow job is a mapping");
            let Some(called) = job
                .get("uses")
                .and_then(Value::as_str)
                .and_then(|uses| uses.strip_prefix("./.github/workflows/"))
            else {
                continue;
            };
            let mut caller = held.clone();
            caller.extend(concurrency_prefixes(job));
            let callee = concurrency_prefixes(
                github_workflow(called)
                    .as_mapping()
                    .expect("called workflow is a mapping"),
            );
            let shared: Vec<&String> = caller.intersection(&callee).collect();
            if !shared.is_empty() {
                let job_name = job_name.as_str().expect("workflow job name is a string");
                deadlocks.push(format!(
                    "{name} job `{job_name}` calls {called} on {shared:?}"
                ));
            }
        }
    }

    assert!(
        deadlocks.is_empty(),
        "a called workflow cannot start while its caller holds the same concurrency group: {deadlocks:?}"
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
fn stress_workflow_is_a_thin_fork_adapter() {
    let text = github_workflow_text("stress.yml");
    for forbidden in [
        "python3",
        "sample_pressure",
        "write_manifest",
        "cargo run",
        "stress-run",
        "just tooling",
        "--test-threads",
        "--flash",
        "--diagnostics",
        "--no-block",
        "--dump-thread-backtrace",
        "--job-timeout-minutes",
        "--expected-filter",
        "--expected-count",
        "--expected-mode",
        "--expected-test-threads",
        "--expected-flash",
        "--expected-no-block",
        "--expected-dump-thread-backtrace",
        "--expected-job-timeout-minutes",
    ] {
        assert!(
            !text.contains(forbidden),
            "stress workflow contains portable implementation detail {forbidden:?}"
        );
    }

    let workflow: Value = serde_yaml_ng::from_str(&text).expect("stress workflow is valid YAML");
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
                .unwrap_or_else(|| panic!("{trigger} is a mapping")),
            "inputs",
        )
        .as_mapping()
        .expect("workflow inputs are a mapping");
        assert_eq!(
            inputs
                .keys()
                .map(|name| name.as_str().expect("input name is a string"))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["count", "filter", "mode", "revision",])
        );
        for name in ["count", "filter", "mode", "revision"] {
            let input = mapping_field(inputs, name)
                .as_mapping()
                .unwrap_or_else(|| panic!("{trigger} input `{name}` is a mapping"));
            assert!(
                !input.contains_key("default"),
                "{trigger} input `{name}` must not own a workflow-local default"
            );
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
    assert_eq!(mapping_field(authorization, "shell").as_str(), Some("bash"));
    let authorization_env = mapping_field(authorization, "env")
        .as_mapping()
        .expect("stress authorization environment is a mapping");
    assert_eq!(authorization_env.len(), 9);
    for (name, expected) in [
        ("ACTOR", "${{ github.actor }}"),
        ("COUNT", "${{ inputs.count || vars.KITHARA_STRESS_COUNT }}"),
        ("ENABLED", "${{ vars.KITHARA_STRESS_ENABLED }}"),
        ("IS_FORK", "${{ github.event.repository.fork }}"),
        ("MAX_COUNT", "${{ vars.KITHARA_STRESS_MAX_COUNT }}"),
        ("OWNER", "${{ github.repository_owner }}"),
        ("REVISION", "${{ inputs.revision || github.sha }}"),
        ("RUNNER_LABELS", "${{ vars.KITHARA_STRESS_RUNNER_LABELS }}"),
        ("TRIGGERING_ACTOR", "${{ github.triggering_actor }}"),
    ] {
        assert_eq!(
            mapping_field(authorization_env, name).as_str(),
            Some(expected)
        );
    }
    let authorization_script = mapping_field(authorization, "run")
        .as_str()
        .expect("authorization is a script");
    for contract in [
        "[[ \"$ENABLED\" == true ]]",
        "[[ \"$IS_FORK\" == true ]]",
        "[[ \"$ACTOR\" == \"$OWNER\" ]]",
        "[[ \"$TRIGGERING_ACTOR\" == \"$OWNER\" ]]",
        "[[ \"$REVISION\" =~ ^[0-9a-fA-F]{40}$ ]]",
        "[[ \"$COUNT\" =~ ^[1-9][0-9]*$ ]]",
        "[[ \"$MAX_COUNT\" =~ ^[1-9][0-9]*$ ]]",
        "10#$COUNT <= 10#$MAX_COUNT",
        "\"self-hosted\", \"linux\", \"x64\", \"kithara\"",
        "<<< \"$RUNNER_LABELS\"",
    ] {
        assert!(
            authorization_script.contains(contract),
            "stress authorization omits {contract:?}"
        );
    }

    let execute = workflow_job(jobs, "execute");
    assert_eq!(job_needs(execute), BTreeSet::from(["authorize".to_owned()]));
    let execute_guard = mapping_field(execute, "if")
        .as_str()
        .expect("execute guard is a string");
    for contract in [
        "github.event.repository.fork == true",
        "github.actor == github.repository_owner",
        "github.triggering_actor == github.repository_owner",
        "vars.KITHARA_STRESS_ENABLED == 'true'",
        "contains(fromJSON(vars.KITHARA_STRESS_RUNNER_LABELS), 'self-hosted')",
        "contains(fromJSON(vars.KITHARA_STRESS_RUNNER_LABELS), 'linux')",
        "contains(fromJSON(vars.KITHARA_STRESS_RUNNER_LABELS), 'x64')",
        "contains(fromJSON(vars.KITHARA_STRESS_RUNNER_LABELS), 'kithara')",
    ] {
        assert!(
            execute_guard.contains(contract),
            "execute rerun guard omits {contract:?}"
        );
    }
    assert_eq!(
        mapping_field(execute, "runs-on").as_str(),
        Some("${{ fromJSON(vars.KITHARA_STRESS_RUNNER_LABELS) }}")
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a workspace root");
    let config: toml::Value = toml::from_str(
        &fs::read_to_string(root.join(".config/xtask.toml")).expect("xtask config is readable"),
    )
    .expect("xtask config is valid TOML");
    let configured_timeout = config["stress"]["workflow_job_timeout_minutes"]
        .as_integer()
        .and_then(|value| u64::try_from(value).ok())
        .expect("stress job timeout is configured");
    assert_eq!(
        mapping_field(execute, "timeout-minutes").as_u64(),
        Some(configured_timeout)
    );
    let execute_outputs = mapping_field(execute, "outputs")
        .as_mapping()
        .expect("execute outputs are a mapping");
    assert_eq!(
        mapping_field(execute_outputs, "artifact-name").as_str(),
        Some("${{ steps.artifact-name.outputs.name }}")
    );
    assert_eq!(
        job_step_names(execute),
        BTreeSet::from([
            "Checkout controller".to_owned(),
            "Checkout subject".to_owned(),
            "Export artifact identity".to_owned(),
            "Run the stress campaign".to_owned(),
            "Upload the raw stress evidence".to_owned(),
        ])
    );

    assert_checkout(
        named_step(execute, "Checkout controller"),
        "controller",
        "${{ job.workflow_repository }}",
        "${{ job.workflow_sha }}",
    );
    assert_checkout(
        named_step(execute, "Checkout subject"),
        "subject",
        "${{ github.repository }}",
        "${{ inputs.revision || github.sha }}",
    );

    assert!(
        step_position(execute, "Checkout subject")
            < step_position(execute, "Run the stress campaign")
    );

    let campaign = named_step(execute, "Run the stress campaign");
    assert_eq!(
        mapping_field(campaign, "working-directory").as_str(),
        Some("controller")
    );
    let campaign_env = mapping_field(campaign, "env")
        .as_mapping()
        .expect("campaign environment is a mapping");
    assert_eq!(campaign_env.len(), 5);
    for (name, expected) in [
        ("CONTROLLER_SHA", "${{ job.workflow_sha }}"),
        ("COUNT", "${{ inputs.count || vars.KITHARA_STRESS_COUNT }}"),
        ("FILTER", "${{ inputs.filter }}"),
        ("MODE", "${{ inputs.mode }}"),
        ("SUBJECT_SHA", "${{ inputs.revision || github.sha }}"),
    ] {
        assert_eq!(mapping_field(campaign_env, name).as_str(), Some(expected));
    }
    let execute_script = mapping_field(campaign, "run")
        .as_str()
        .expect("campaign command is a script")
        .trim();
    assert_eq!(execute_script, STRESS_EXECUTE_COMMAND);
    assert_eq!(execute_script.matches("just ci stress").count(), 1);

    let raw_upload = named_step(execute, "Upload the raw stress evidence");
    assert_uploads(raw_upload, &[STRESS_RAW_DIR]);
    let raw_upload_inputs = mapping_field(raw_upload, "with")
        .as_mapping()
        .expect("raw upload inputs are a mapping");
    assert_eq!(
        mapping_field(raw_upload_inputs, "name").as_str(),
        Some("stress-raw-${{ github.run_id }}-${{ github.run_attempt }}")
    );
    assert_eq!(
        mapping_field(raw_upload_inputs, "retention-days").as_u64(),
        Some(14)
    );
    assert_eq!(
        mapping_field(raw_upload_inputs, "if-no-files-found").as_str(),
        Some("error")
    );
    let artifact_name = named_step(execute, "Export artifact identity");
    assert_always(artifact_name);
    assert_eq!(
        mapping_field(artifact_name, "id").as_str(),
        Some("artifact-name")
    );
    let artifact_script = mapping_field(artifact_name, "run")
        .as_str()
        .expect("artifact identity command is a script");
    assert!(artifact_script.contains("[[ \"$UPLOAD_OUTCOME\" == success ]] || exit 1"));
    assert!(artifact_script.contains("name=stress-raw-${GITHUB_RUN_ID}-${GITHUB_RUN_ATTEMPT}"));

    let report = workflow_job(jobs, "report");
    assert_eq!(job_needs(report), BTreeSet::from(["execute".to_owned()]));
    assert_eq!(
        mapping_field(report, "runs-on").as_str(),
        Some("ubuntu-latest")
    );
    assert_eq!(
        mapping_field(report, "if").as_str(),
        Some("${{ always() && needs.execute.result != 'skipped' }}")
    );
    assert_eq!(
        job_step_names(report),
        BTreeSet::from([
            "Add the stress summary".to_owned(),
            "Checkout controller".to_owned(),
            "Download the raw stress evidence".to_owned(),
            "Install just".to_owned(),
            "Upload the stress evidence".to_owned(),
            "Verify and render the stress evidence".to_owned(),
        ])
    );

    assert_checkout(
        named_step(report, "Checkout controller"),
        "controller",
        "${{ job.workflow_repository }}",
        "${{ job.workflow_sha }}",
    );
    let download = named_step(report, "Download the raw stress evidence");
    assert_eq!(
        mapping_field(download, "uses").as_str(),
        Some(DOWNLOAD_ARTIFACT)
    );
    let download_inputs = mapping_field(download, "with")
        .as_mapping()
        .expect("artifact download inputs are a mapping");
    assert_eq!(
        mapping_field(download_inputs, "name").as_str(),
        Some("${{ needs.execute.outputs.artifact-name }}")
    );
    assert_eq!(mapping_field(download_inputs, "path").as_str(), Some("raw"));

    let report_install = named_step(report, "Install just");
    assert_always(report_install);
    assert_eq!(
        mapping_field(report_install, "uses").as_str(),
        Some(INSTALL_ACTION)
    );

    let verifier = named_step(report, "Verify and render the stress evidence");
    assert_always(verifier);
    assert_eq!(
        mapping_field(verifier, "working-directory").as_str(),
        Some("controller")
    );
    let verifier_env = mapping_field(verifier, "env")
        .as_mapping()
        .expect("verifier environment is a mapping");
    assert_eq!(verifier_env.len(), 6);
    for (name, expected) in [
        ("CONTROLLER_SHA", "${{ job.workflow_sha }}"),
        ("COUNT", "${{ inputs.count || vars.KITHARA_STRESS_COUNT }}"),
        ("EXECUTE_RESULT", "${{ needs.execute.result }}"),
        ("FILTER", "${{ inputs.filter }}"),
        ("MODE", "${{ inputs.mode }}"),
        ("SUBJECT_SHA", "${{ inputs.revision || github.sha }}"),
    ] {
        assert_eq!(mapping_field(verifier_env, name).as_str(), Some(expected));
    }
    let report_script = mapping_field(verifier, "run")
        .as_str()
        .expect("report command is a script")
        .trim();
    assert_eq!(report_script, STRESS_REPORT_COMMAND);
    assert_eq!(report_script.matches("just ci stress-report").count(), 1);

    let summary = named_step(report, "Add the stress summary");
    assert_always(summary);
    let summary_script = mapping_field(summary, "run")
        .as_str()
        .expect("summary command is a string");
    assert!(summary_script.contains("[[ -s \"$GITHUB_WORKSPACE/target/stress-report.md\" ]]"));
    assert!(
        summary_script.contains(
            "cat \"$GITHUB_WORKSPACE/target/stress-report.md\" >> \"$GITHUB_STEP_SUMMARY\""
        )
    );

    let evidence_upload = named_step(report, "Upload the stress evidence");
    assert_uploads(evidence_upload, &["raw", "target/stress-report.md"]);
    assert_eq!(
        mapping_field(
            mapping_field(evidence_upload, "with")
                .as_mapping()
                .expect("evidence upload inputs are a mapping"),
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
    let prekill = configured_stress_prekill_secs(root);
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
    let prekill = configured_stress_prekill_secs(root);
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
                if seconds.saturating_add(30) > prekill {
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
        "declared test timeouts leave less than 30s before the configured pre-kill snapshot: {excessive:?}"
    );
    assert!(prekill >= observed_max + 30);
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
