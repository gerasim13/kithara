use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::Value;

use super::{
    AttemptDossier, MAX_SIGNATURE_EXAMPLES, SignatureCluster, add_signature,
    attempt::{
        AttemptKey, AttemptMetadata, AttemptOutcome, foreign_run, outcome_for, render_key,
        render_test,
    },
    normalize_signature, render_clusters, wait_signatures,
};
use crate::common::project::StressEvidenceConfig;

const MAX_ENVELOPE_BYTES: u64 = 4 * 1_024 * 1_024;
const MAX_ENVELOPE_DIRECTORY_ENTRIES: usize = 100_000;

#[derive(Debug, Deserialize)]
struct AttemptEnvelope {
    schema: String,
    label: String,
    diagnostic: String,
    nextest: EnvelopeNextest,
    context: Value,
}

#[derive(Debug, Deserialize)]
struct EnvelopeNextest {
    run_id: Option<String>,
    binary_id: Option<String>,
    attempt_id: Option<String>,
    test_name: Option<String>,
    stress_current: Option<String>,
}

#[derive(Clone, Copy)]
pub(super) struct Input<'a> {
    outcomes: &'a BTreeMap<AttemptKey, AttemptOutcome>,
    expected: &'a BTreeSet<AttemptKey>,
    run_id: Option<&'a str>,
    evidence: &'a StressEvidenceConfig,
}

struct EnvelopeFiles {
    paths: Vec<PathBuf>,
    invalid: usize,
    limit_exceeded: bool,
}

impl<'a> Input<'a> {
    pub(super) const fn new(
        outcomes: &'a BTreeMap<AttemptKey, AttemptOutcome>,
        expected: &'a BTreeSet<AttemptKey>,
        run_id: Option<&'a str>,
        evidence: &'a StressEvidenceConfig,
    ) -> Self {
        Self {
            outcomes,
            expected,
            run_id,
            evidence,
        }
    }
}

pub(super) fn append(
    out: &mut String,
    dir: &Path,
    input: Input<'_>,
    wait_clusters: &mut BTreeMap<String, SignatureCluster>,
    dossiers: &mut BTreeMap<AttemptKey, AttemptDossier>,
) -> bool {
    let Some(files) = envelope_files(dir) else {
        let _ = writeln!(
            out,
            "\n## Attempt-envelope progress signatures\n\nThe requested envelope directory could not be read."
        );
        return false;
    };
    let mut invalid = files.invalid;
    let mut clusters = BTreeMap::new();
    let mut matched = BTreeSet::new();
    let mut foreign = 0usize;
    for path in files.paths {
        let Some(text) = read_envelope(&path) else {
            invalid = invalid.saturating_add(1);
            continue;
        };
        let Ok(envelope) = serde_json::from_str::<AttemptEnvelope>(&text) else {
            invalid = invalid.saturating_add(1);
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            invalid = invalid.saturating_add(1);
            continue;
        };
        let payload = match input
            .evidence
            .envelope_text_field
            .as_deref()
            .and_then(|field| value.get(field))
        {
            None | Some(Value::Null) => None,
            Some(Value::String(payload)) => Some(payload.clone()),
            Some(_) => {
                invalid = invalid.saturating_add(1);
                continue;
            }
        };
        if input.evidence.envelope_schema.as_deref() != Some(envelope.schema.as_str()) {
            invalid = invalid.saturating_add(1);
            continue;
        }
        let Ok(attempt) = AttemptMetadata::from_fields(
            envelope.nextest.binary_id.as_deref(),
            envelope.nextest.test_name.as_deref(),
            envelope.nextest.stress_current.as_deref(),
            envelope.nextest.run_id.as_deref(),
            envelope.nextest.attempt_id.as_deref(),
        ) else {
            invalid = invalid.saturating_add(1);
            continue;
        };
        if foreign_run(&attempt, input.run_id) {
            foreign = foreign.saturating_add(1);
            continue;
        }

        matched.insert(attempt.key.clone());
        let test = render_test(&attempt.key);
        let outcome = outcome_for(&attempt, input.outcomes);
        let context = envelope.context.to_string();
        let signature =
            normalize_signature(&format!("{}: {}", envelope.label, envelope.diagnostic));
        if outcome == AttemptOutcome::Failed
            && let Some(dossier) = dossiers.get_mut(&attempt.key)
        {
            dossier.envelopes.insert(signature.clone());
        }
        add_signature(
            &mut clusters,
            signature,
            &attempt.display,
            &test,
            outcome,
            Some(context.as_str()),
        );
        if let Some(payload) = payload {
            for signature in wait_signatures(&payload, input.evidence) {
                if outcome == AttemptOutcome::Failed
                    && let Some(dossier) = dossiers.get_mut(&attempt.key)
                {
                    dossier.wait_graph.insert(signature.clone());
                }
                add_signature(
                    wait_clusters,
                    signature,
                    &attempt.display,
                    &test,
                    outcome,
                    None,
                );
            }
        }
    }
    render_clusters(
        out,
        "Attempt-envelope progress signatures",
        &clusters,
        "These pair the stalled source location with the last observed progress point and the exact nextest attempt.",
    );
    if foreign > 0 {
        let _ = writeln!(
            out,
            "\nIgnored `{foreign}` envelope records from a different nextest run."
        );
    }
    if invalid > 0 {
        let _ = writeln!(
            out,
            "\nEvidence problem: `{invalid}` envelope artifacts were unreadable, oversized, malformed, or carried invalid nextest metadata."
        );
    }
    if files.limit_exceeded {
        let _ = writeln!(
            out,
            "\nEvidence problem: the envelope directory exceeds the deterministic limit of `{MAX_ENVELOPE_DIRECTORY_ENTRIES}` entries. The raw directory was left untouched."
        );
    }
    let missing = input.expected.difference(&matched).collect::<Vec<_>>();
    if !missing.is_empty() {
        let examples = missing
            .iter()
            .take(MAX_SIGNATURE_EXAMPLES)
            .map(|key| super::markdown_cell(&render_key(key)))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "\nEvidence problem: `{}` timeout-class failed attempts had no exact same-run attempt envelope: {examples}",
            missing.len(),
        );
    }
    invalid == 0 && !files.limit_exceeded && missing.is_empty()
}

fn envelope_files(dir: &Path) -> Option<EnvelopeFiles> {
    let entries = fs::read_dir(dir).ok()?;
    let mut paths = Vec::new();
    let mut invalid = 0usize;
    let mut limit_exceeded = false;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_ENVELOPE_DIRECTORY_ENTRIES {
            limit_exceeded = true;
            break;
        }
        let Ok(entry) = entry else {
            invalid = invalid.saturating_add(1);
            continue;
        };
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            paths.push(path);
        }
    }
    paths.sort();
    Some(EnvelopeFiles {
        paths,
        invalid,
        limit_exceeded,
    })
}

fn read_envelope(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    if length > MAX_ENVELOPE_BYTES {
        return None;
    }
    let capacity = usize::try_from(length).ok()?;
    let mut bytes = Vec::with_capacity(capacity.checked_add(1)?);
    let max_bytes = usize::try_from(MAX_ENVELOPE_BYTES).ok()?;
    file.take(MAX_ENVELOPE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > max_bytes {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence() -> StressEvidenceConfig {
        StressEvidenceConfig {
            envelope_schema: Some("demo.hang.v1".to_owned()),
            envelope_text_field: Some("wait_graph".to_owned()),
            dump_marker: Some("[wait dump]".to_owned()),
            primitive_marker: Some("created_at=".to_owned()),
            holder_marker: Some("held by".to_owned()),
            wait_marker: Some("WAITING:".to_owned()),
            ..StressEvidenceConfig::default()
        }
    }

    fn key() -> AttemptKey {
        AttemptKey {
            suite: "demo::tests".to_owned(),
            name: "seek".to_owned(),
            iteration: 0,
        }
    }

    fn dossier(key: AttemptKey) -> BTreeMap<AttemptKey, AttemptDossier> {
        BTreeMap::from([(
            key,
            AttemptDossier {
                display: "failed-attempt".to_owned(),
                ..AttemptDossier::default()
            },
        )])
    }

    #[test]
    fn unknown_schema_is_incomplete() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(
            temp.path().join("hang.json"),
            r#"{"schema":"other","label":"stalled","diagnostic":"none"}"#,
        )
        .expect("write hang fixture");
        let mut markdown = String::new();
        let outcomes = BTreeMap::new();
        let expected = BTreeSet::new();
        let evidence = evidence();

        assert!(!append(
            &mut markdown,
            temp.path(),
            Input::new(&outcomes, &expected, None, &evidence),
            &mut BTreeMap::new(),
            &mut BTreeMap::new(),
        ));
        assert!(markdown.contains("`1` envelope artifacts"), "{markdown}");
    }

    #[test]
    fn oversized_envelope_is_rejected_without_unbounded_reading() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("hang.json");
        let file = File::create(&path).expect("create fixture");
        file.set_len(MAX_ENVELOPE_BYTES + 1).expect("size fixture");

        assert!(read_envelope(&path).is_none());
    }

    #[test]
    fn timeout_requires_an_exact_same_run_envelope() {
        let temp = tempfile::tempdir().expect("tempdir");
        let key = key();
        let expected = BTreeSet::from([key.clone()]);
        let outcomes = BTreeMap::from([(key.clone(), AttemptOutcome::Failed)]);
        let mut dossiers = dossier(key);
        let mut markdown = String::new();
        let evidence = evidence();

        assert!(!append(
            &mut markdown,
            temp.path(),
            Input::new(&outcomes, &expected, Some("run"), &evidence),
            &mut BTreeMap::new(),
            &mut dossiers,
        ));
        assert!(
            markdown.contains("no exact same-run attempt envelope"),
            "{markdown}"
        );
    }

    #[test]
    fn configured_envelope_text_joins_the_failed_attempt() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(
            temp.path().join("hang.json"),
            r##"{
  "schema":"demo.hang.v1",
  "label":"test-timeout",
  "diagnostic":"hard-timeout",
  "nextest":{
    "run_id":"run",
    "binary_id":"demo::tests",
    "test_name":"seek",
    "stress_current":"0"
  },
  "context":{},
  "wait_graph":"[wait dump] hard-timeout\n#7 Mutex created_at=crates/demo/src/state.rs:10\nheld by worker\nWAITING: reader"
}"##,
        )
        .expect("write hang fixture");
        let key = key();
        let expected = BTreeSet::from([key.clone()]);
        let outcomes = BTreeMap::from([(key.clone(), AttemptOutcome::Failed)]);
        let mut dossiers = dossier(key);
        let mut waits = BTreeMap::new();
        let mut markdown = String::new();
        let evidence = evidence();

        assert!(append(
            &mut markdown,
            temp.path(),
            Input::new(&outcomes, &expected, Some("run"), &evidence),
            &mut waits,
            &mut dossiers,
        ));

        assert_eq!(waits.len(), 1);
        assert_eq!(
            dossiers.values().next().expect("dossier").wait_graph.len(),
            1
        );
        assert!(
            markdown.contains("demo::tests seek @stress-0"),
            "{markdown}"
        );
    }

    #[test]
    fn foreign_or_duplicate_metadata_does_not_satisfy_expected_attempt() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(
            temp.path().join("foreign.json"),
            r#"{"schema":"demo.hang.v1","label":"stalled","diagnostic":"none","nextest":{"run_id":"foreign","binary_id":"demo::tests","attempt_id":"opaque/retry#2","test_name":"seek","stress_current":"0"},"context":{},"wait_graph":null}"#,
        )
        .expect("write foreign fixture");
        fs::write(
            temp.path().join("duplicate.json"),
            r#"{"schema":"demo.hang.v1","label":"stalled","diagnostic":"none","nextest":{"run_id":"run","binary_id":"demo::tests","binary_id":"other","test_name":"seek","stress_current":"0"},"context":{},"wait_graph":null}"#,
        )
        .expect("write duplicate fixture");
        let key = key();
        let mut markdown = String::new();
        let outcomes = BTreeMap::from([(key.clone(), AttemptOutcome::Failed)]);
        let expected = BTreeSet::from([key]);
        let evidence = evidence();

        assert!(!append(
            &mut markdown,
            temp.path(),
            Input::new(&outcomes, &expected, Some("run"), &evidence),
            &mut BTreeMap::new(),
            &mut BTreeMap::new(),
        ));
        assert!(
            markdown.contains("Ignored `1` envelope records"),
            "{markdown}"
        );
        assert!(markdown.contains("`1` envelope artifacts"), "{markdown}");
        assert!(
            markdown.contains("no exact same-run attempt envelope"),
            "{markdown}"
        );
    }
}
