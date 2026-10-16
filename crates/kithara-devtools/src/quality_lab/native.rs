use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, ensure};
use serde::Deserialize;
use serde_json::{Deserializer, Value};

use super::adapter::{InvocationSpec, ReportKind};
use crate::consts;

/// Judges one invocation's report. `Some` describes what the report found; a
/// render carries no verdict of its own, but an empty render means the render
/// died quietly and the run would publish a blank page.
pub(super) fn validate_report(path: &Path, invocation: &InvocationSpec) -> Result<Option<String>> {
    let text =
        fs::read_to_string(path).with_context(|| format!("read report: {}", path.display()))?;
    match &invocation.report {
        ReportKind::CyclomaticGrowth { baseline, root } => {
            let baseline = fs::read_to_string(baseline)
                .with_context(|| format!("read baseline report: {}", baseline.display()))?;
            cyclomatic_growth(&text, &baseline, root)
        }
        ReportKind::Json => {
            serde_json::from_str::<Value>(&text).context("parse JSON report")?;
            Ok(None)
        }
        ReportKind::JsonStream => cargo_dupes_findings(&text),
        ReportKind::Markdown => {
            ensure!(!text.trim().is_empty(), "markdown report is empty");
            Ok(None)
        }
        ReportKind::RustqualJson => rustqual_findings(&text),
    }
}

#[derive(Deserialize)]
struct CrapReport {
    entries: Vec<CrapEntry>,
}

#[derive(Deserialize)]
struct CrapEntry {
    file: PathBuf,
    function: String,
    cyclomatic: f64,
}

/// Functions above the cyclomatic limit that are new since the baseline or
/// grew there. A function is named by its file and path, so a move reads as
/// new. Both reports name files by absolute path, and one measured in another
/// checkout would make every function new, so that is refused instead.
fn cyclomatic_growth(text: &str, baseline: &str, root: &Path) -> Result<Option<String>> {
    let current: CrapReport = serde_json::from_str(text).context("parse cargo-crap JSON report")?;
    let baseline: CrapReport =
        serde_json::from_str(baseline).context("parse cargo-crap baseline report")?;
    let mut before = HashMap::with_capacity(baseline.entries.len());
    for entry in baseline.entries {
        let key = (relative(&entry.file, root, "baseline")?, entry.function);
        let cyclomatic = before.entry(key).or_insert(entry.cyclomatic);
        *cyclomatic = entry.cyclomatic.max(*cyclomatic);
    }
    let mut grown = Vec::new();
    for entry in current.entries {
        if entry.cyclomatic <= consts::CYCLOMATIC_LIMIT {
            continue;
        }
        let key = (relative(&entry.file, root, "report")?, entry.function);
        let name = format!("{}::{}", key.0.display(), key.1);
        match before.get(&key) {
            None => grown.push(format!("{name} (new, {})", entry.cyclomatic)),
            Some(&was) if entry.cyclomatic > was => {
                grown.push(format!("{name} ({was} -> {})", entry.cyclomatic));
            }
            Some(_) => {}
        }
    }
    Ok((!grown.is_empty()).then(|| {
        format!(
            "cyclomatic complexity above {} is new or grew in {} function(s): {}",
            consts::CYCLOMATIC_LIMIT,
            grown.len(),
            grown.join(", ")
        )
    }))
}

fn relative(file: &Path, root: &Path, report: &str) -> Result<PathBuf> {
    file.strip_prefix(root)
        .map(Path::to_path_buf)
        .with_context(|| {
            format!(
                "{report} measured another checkout than {}: {}",
                root.display(),
                file.display()
            )
        })
}

fn rustqual_findings(text: &str) -> Result<Option<String>> {
    let report: Value = serde_json::from_str(text).context("parse rustqual JSON report")?;
    let summary = report
        .get("summary")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("rustqual JSON report omits summary"))?;
    [
        "tq_no_assertion_warnings",
        "tq_no_sut_warnings",
        "tq_untested_warnings",
        "tq_uncovered_warnings",
        "tq_untested_logic_warnings",
    ]
    .into_iter()
    .filter_map(|field| match summary.get(field).and_then(Value::as_u64) {
        Some(0) => None,
        Some(count) => Some(Ok(format!("{field}={count}"))),
        None => Some(Err(anyhow!("rustqual summary omits {field}"))),
    })
    .collect::<Result<Vec<_>>>()
    .map(|warnings| (!warnings.is_empty()).then(|| warnings.join(", ")))
}

fn cargo_dupes_findings(text: &str) -> Result<Option<String>> {
    let mut values = Deserializer::from_str(text).into_iter::<Value>();
    let stats = values
        .next()
        .transpose()
        .context("parse cargo-dupes statistics")?
        .ok_or_else(|| anyhow!("cargo-dupes JSON stream is empty"))?;
    for value in values {
        value.context("parse cargo-dupes JSON stream")?;
    }
    let stats = stats
        .as_object()
        .ok_or_else(|| anyhow!("cargo-dupes statistics must be the first JSON object"))?;
    let exact = stats
        .get("sub_exact_groups")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("cargo-dupes statistics omit sub_exact_groups"))?;
    let near = stats
        .get("sub_near_groups")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("cargo-dupes statistics omit sub_near_groups"))?;
    Ok((exact + near > 0)
        .then(|| format!("{exact} exact and {near} near sub-function duplicate groups")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_dupes_uses_only_sub_function_group_counts() {
        let clean = r#"{"exact_groups":9,"near_groups":7,"sub_exact_groups":0,"sub_near_groups":0}
[]
[]"#;
        let findings = r#"{"exact_groups":0,"near_groups":0,"sub_exact_groups":1,"sub_near_groups":0}
[{"kind":"sub-exact"}]
[]"#;

        assert_eq!(cargo_dupes_findings(clean).expect("clean stream"), None);
        assert!(
            cargo_dupes_findings(findings)
                .expect("findings stream")
                .is_some()
        );
    }

    #[test]
    fn rustqual_uses_only_test_quality_counts() {
        let clean = r#"{
            "summary": {
                "violations": 900,
                "tq_no_assertion_warnings": 0,
                "tq_no_sut_warnings": 0,
                "tq_untested_warnings": 0,
                "tq_uncovered_warnings": 0,
                "tq_untested_logic_warnings": 0
            }
        }"#;
        let findings = clean.replace("\"tq_no_sut_warnings\": 0", "\"tq_no_sut_warnings\": 1");

        assert_eq!(
            rustqual_findings(clean).expect("clean rustqual report"),
            None
        );
        assert_eq!(
            rustqual_findings(&findings).expect("rustqual findings"),
            Some("tq_no_sut_warnings=1".to_owned())
        );
    }

    fn report(entries: &[(&str, &str, f64)]) -> String {
        let entries = entries
            .iter()
            .map(|(file, function, cyclomatic)| {
                serde_json::json!({
                    "file": format!("/checkout/{file}"),
                    "function": function,
                    "cyclomatic": cyclomatic,
                    "crap": 1.0,
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({ "entries": entries }).to_string()
    }

    fn growth(current: &[(&str, &str, f64)], baseline: &[(&str, &str, f64)]) -> Option<String> {
        cyclomatic_growth(&report(current), &report(baseline), Path::new("/checkout"))
            .expect("both reports parse")
    }

    #[test]
    fn complexity_above_the_limit_gates_only_where_it_is_new_or_grew() {
        let limit = consts::CYCLOMATIC_LIMIT;
        let baseline = [
            ("src/a.rs", "steady", limit + 5.0),
            ("src/a.rs", "grows", limit + 1.0),
            ("src/a.rs", "shrinks", limit + 9.0),
            ("src/a.rs", "crosses", limit),
        ];

        assert_eq!(
            growth(
                &[
                    ("src/a.rs", "steady", limit + 5.0),
                    ("src/a.rs", "shrinks", limit + 2.0),
                    ("src/b.rs", "small", limit),
                ],
                &baseline,
            ),
            None,
            "unchanged, shrinking and at-limit functions pass"
        );
        let findings = growth(
            &[
                ("src/a.rs", "grows", limit + 3.0),
                ("src/a.rs", "crosses", limit + 1.0),
                ("src/b.rs", "fresh", limit + 1.0),
            ],
            &baseline,
        )
        .expect("growth above the limit is a finding");
        assert!(findings.contains("3 function(s)"), "{findings}");
        let grew = format!("src/a.rs::grows ({} -> {})", limit + 1.0, limit + 3.0);
        let fresh = format!("src/b.rs::fresh (new, {})", limit + 1.0);
        assert!(findings.contains(&grew), "{findings}");
        assert!(findings.contains(&fresh), "{findings}");
    }

    #[test]
    fn a_baseline_from_another_checkout_is_refused() {
        let foreign =
            r#"{"entries":[{"file":"/elsewhere/src/a.rs","function":"f","cyclomatic":1.0}]}"#;
        let error = cyclomatic_growth(&report(&[]), foreign, Path::new("/checkout"))
            .expect_err("a foreign baseline would make every function new");
        assert!(error.to_string().contains("another checkout"), "{error}");
    }
}
