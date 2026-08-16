use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use clap::Args;
use serde_json::Value;

struct Consts;
impl Consts {
    /// Rows of the CRAP table inlined before the reader is sent to the
    /// artifact. The full table runs to five figures of lines, and a step
    /// summary is capped at a megabyte.
    const CRAP_ROWS: usize = 120;

    const CRAP_DIRECTORY: &'static str = "cargo-crap";
    const CRAP_REPORT: &'static str = "report.md";
    const HEALTH_REPORT: &'static str = "health-report.md";
    const METRICS: &'static str = "metrics.json";
    /// Where the health report stops being a verdict and starts being logs.
    const STAGE_DETAILS: &'static str = "## Stage details";
    const TOP_CONTOURS: usize = 10;
}

#[derive(Debug, Args)]
pub struct CiReportArgs {
    /// Directory holding the quality artifacts of one run.
    #[arg(long, value_name = "DIR")]
    pub artifacts: PathBuf,
    /// Rows of the CRAP table to inline before pointing at the artifact.
    #[arg(long, default_value_t = Consts::CRAP_ROWS)]
    pub crap_rows: usize,
}

pub(crate) fn run(args: &CiReportArgs) -> Result<()> {
    print!("{}", render(&args.artifacts, args.crap_rows)?);
    Ok(())
}

fn render(artifacts: &Path, crap_rows: usize) -> Result<String> {
    let mut out = String::new();
    out.push_str(&health(artifacts)?);
    out.push_str(&coverage_risk(artifacts, crap_rows)?);
    out.push_str(&architecture(artifacts)?);
    Ok(out)
}

fn health(artifacts: &Path) -> Result<String> {
    let Some(report) = find(artifacts, &|path| named(path, Consts::HEALTH_REPORT))? else {
        return Ok(missing("Workspace health", "health-report"));
    };
    let text = read(&report)?;
    let summary = text
        .split_once(Consts::STAGE_DETAILS)
        .map_or(text.as_str(), |(before, _)| before);
    Ok(format!("{}\n", summary.trim_end()))
}

fn coverage_risk(artifacts: &Path, rows: usize) -> Result<String> {
    let Some(report) = find(artifacts, &|path| {
        named(path, Consts::CRAP_REPORT) && parent_named(path, Consts::CRAP_DIRECTORY)
    })?
    else {
        return Ok(missing("Coverage risk (CRAP)", "coverage-risk"));
    };
    let text = read(&report)?;
    let mut out = String::from("\n## Coverage risk (CRAP)\n\n");
    for line in text.lines().take(rows) {
        out.push_str(line);
        out.push('\n');
    }
    if text.lines().count() > rows {
        out.push_str("\nTruncated here; the whole table is in the `coverage-risk` artifact.\n");
    }
    Ok(out)
}

fn architecture(artifacts: &Path) -> Result<String> {
    let Some(metrics) = find(artifacts, &|path| named(path, Consts::METRICS))? else {
        return Ok(missing("Architecture complexity", "architecture"));
    };
    let text = read(&metrics)?;
    let value: Value =
        serde_json::from_str(&text).with_context(|| format!("parse {}", metrics.display()))?;
    let mut out = String::from("\n## Architecture complexity\n\n");
    let _ = writeln!(
        out,
        "- Architecture complexity index: {}",
        index(&value, "architecture_complexity_index")
    );
    let _ = writeln!(
        out,
        "- Including candidate contours: {}",
        index(&value, "including_candidates_complexity_index")
    );
    let mut contours: Vec<(&String, f64)> = value
        .get("contours")
        .and_then(Value::as_object)
        .map(|contours| {
            contours
                .iter()
                .map(|(name, contour)| (name, aci(contour)))
                .collect()
        })
        .unwrap_or_default();
    if contours.is_empty() {
        return Ok(out);
    }
    // Ranked by the metric itself, so the worst contour is the first row a
    // reader lands on; ties keep the file's own order.
    contours.sort_by(|left, right| right.1.total_cmp(&left.1));
    out.push_str("\n| Contour | ACI |\n|---|---:|\n");
    for (name, score) in contours.iter().take(Consts::TOP_CONTOURS) {
        let _ = writeln!(out, "| `{name}` | {score} |");
    }
    Ok(out)
}

/// A section whose input never arrived says so. Dropping it silently would
/// read as "nothing to report" from a run that reported nothing.
fn missing(section: &str, artifact: &str) -> String {
    format!("\n## {section}\n\nNo `{artifact}` artifact in this run.\n")
}

fn index(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_f64)
        .map_or_else(|| "unavailable".to_owned(), |score| score.to_string())
}

fn aci(contour: &Value) -> f64 {
    contour
        .get("architecture_complexity_index")
        .and_then(Value::as_f64)
        .unwrap_or_default()
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
}

fn named(path: &Path, name: &str) -> bool {
    path.file_name().is_some_and(|found| found == name)
}

fn parent_named(path: &Path, name: &str) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|found| found == name)
}

/// Artifact layouts differ by how many paths their upload listed, so the
/// report locates its inputs by name rather than by a path the workflow would
/// have to keep in step.
fn find(root: &Path, accept: &impl Fn(&Path) -> bool) -> Result<Option<PathBuf>> {
    if !root.is_dir() {
        return Ok(None);
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(root)
        .with_context(|| format!("read {}", root.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("walk {}", root.display()))?;
    entries.sort();
    for entry in &entries {
        if entry.is_dir() {
            if let Some(found) = find(entry, accept)? {
                return Ok(Some(found));
            }
        } else if accept(entry) {
            return Ok(Some(entry.clone()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create artifact directory");
        }
        fs::write(path, contents).expect("write artifact");
    }

    #[test]
    fn health_section_drops_the_stage_logs() {
        let temp = tempdir().expect("tempdir");
        write(
            &temp.path().join("health-report/health-report.md"),
            "# health report\n\n## Summary\n\n| 1 | orphans | FAIL |\n\n## Stage details\n\nlog tail\n",
        );

        let report = health(temp.path()).expect("health section");

        assert!(!report.contains("log tail"));
    }

    #[test]
    fn health_section_keeps_the_stage_table() {
        let temp = tempdir().expect("tempdir");
        write(
            &temp.path().join("health-report/health-report.md"),
            "# health report\n\n## Summary\n\n| 1 | orphans | FAIL |\n\n## Stage details\n\nlog tail\n",
        );

        let report = health(temp.path()).expect("health section");

        assert!(report.contains("| 1 | orphans | FAIL |"));
    }

    #[test]
    fn coverage_risk_section_caps_the_table() {
        let temp = tempdir().expect("tempdir");
        let rows = (0..40).fold(String::new(), |mut table, row| {
            let _ = writeln!(table, "| row {row} |");
            table
        });
        write(
            &temp.path().join("quality-lab/rev/cargo-crap/report.md"),
            &rows,
        );

        let report = coverage_risk(temp.path(), 5).expect("coverage-risk section");

        assert!(!report.contains("| row 6 |"));
    }

    #[test]
    fn coverage_risk_section_points_at_the_artifact_when_capped() {
        let temp = tempdir().expect("tempdir");
        write(
            &temp.path().join("quality-lab/rev/cargo-crap/report.md"),
            "| a |\n| b |\n| c |\n",
        );

        let report = coverage_risk(temp.path(), 1).expect("coverage-risk section");

        assert!(report.contains("coverage-risk` artifact"));
    }

    #[test]
    fn coverage_risk_section_reads_only_the_crap_report() {
        let temp = tempdir().expect("tempdir");
        write(&temp.path().join("similarity/report.md"), "duplication\n");

        let report = coverage_risk(temp.path(), 10).expect("coverage-risk section");

        assert!(report.contains("No `coverage-risk` artifact"));
    }

    #[test]
    fn architecture_section_ranks_the_worst_contour_first() {
        let temp = tempdir().expect("tempdir");
        write(
            &temp.path().join("architecture/rev/metrics.json"),
            r#"{
                "architecture_complexity_index": 15.6,
                "including_candidates_complexity_index": 15.6,
                "contours": {
                    "crates/quiet": {"architecture_complexity_index": 1.0},
                    "crates/loud": {"architecture_complexity_index": 9.0}
                }
            }"#,
        );

        let report = architecture(temp.path()).expect("architecture section");
        let rows: Vec<&str> = report
            .lines()
            .filter(|line| line.starts_with("| `crates/"))
            .collect();

        assert_eq!(rows.first().copied(), Some("| `crates/loud` | 9 |"));
    }

    #[test]
    fn architecture_section_states_the_workspace_index() {
        let temp = tempdir().expect("tempdir");
        write(
            &temp.path().join("architecture/rev/metrics.json"),
            r#"{"architecture_complexity_index": 15.6, "contours": {}}"#,
        );

        let report = architecture(temp.path()).expect("architecture section");

        assert!(report.contains("- Architecture complexity index: 15.6"));
    }

    #[test]
    fn a_missing_artifact_is_stated_rather_than_dropped() {
        let temp = tempdir().expect("tempdir");

        let report = render(temp.path(), 10).expect("report");

        assert!(report.contains("No `health-report` artifact in this run."));
    }
}
