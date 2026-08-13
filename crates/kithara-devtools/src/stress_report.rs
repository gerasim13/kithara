//! Builds a bounded Markdown summary from a nextest stress JUnit report.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::Args;

use crate::junit::{CaseTiming, parse_junit};

const DEFAULT_JUNIT: &str = "target/nextest/stress/junit.xml";
const DEFAULT_OUTPUT: &str = "target/stress-report.md";
const MAX_FAILURE_ROWS: usize = 100;
const MAX_ITERATIONS_PER_TEST: usize = 20;
const PERCENT_SCALE: usize = 100;
const PERCENT_HUNDREDTHS: usize = PERCENT_SCALE * PERCENT_SCALE;

#[derive(Debug, Args)]
#[non_exhaustive]
pub struct StressReportArgs {
    /// JUnit emitted by the nextest stress profile.
    #[arg(long, default_value = DEFAULT_JUNIT)]
    junit: PathBuf,
    /// Markdown summary destination.
    #[arg(long, default_value = DEFAULT_OUTPUT)]
    output: PathBuf,
    /// Number of stress iterations requested from nextest.
    #[arg(long)]
    expected_count: usize,
    /// Write an explicit incomplete report when nextest produced no JUnit.
    #[arg(long)]
    allow_missing: bool,
}

#[derive(Debug, Default)]
struct TestStats {
    attempts: usize,
    failures: usize,
    max_secs: f64,
    observed_iterations: BTreeSet<usize>,
    failed_iterations: BTreeSet<usize>,
}

/// Summarizes one nextest stress run.
///
/// # Errors
///
/// Returns an error when the input is absent without `--allow-missing`, is
/// invalid, the expected count is zero, or the output cannot be written.
pub(crate) fn run(args: &StressReportArgs) -> Result<()> {
    if args.expected_count == 0 {
        bail!("expected-count must be greater than zero");
    }
    let xml = match fs::read_to_string(&args.junit) {
        Ok(xml) => xml,
        Err(error) if args.allow_missing && error.kind() == std::io::ErrorKind::NotFound => {
            let markdown = render_missing(args.expected_count, &args.junit);
            write_report(&args.output, &markdown)?;
            return Ok(());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read stress JUnit at {}", args.junit.display()));
        }
    };
    let cases = parse_junit(&xml)
        .with_context(|| format!("parse stress JUnit at {}", args.junit.display()))?;
    let markdown = render(&cases, args.expected_count);
    write_report(&args.output, &markdown)?;
    Ok(())
}

fn render_missing(expected_count: usize, junit: &Path) -> String {
    format!(
        "# Stress evidence\n\n- Result: **NO JUNIT**\n- Requested iterations: `{expected_count}`\n- JUnit path: `{}`\n\nNextest did not reach the point where it could write per-iteration evidence. Inspect the primary step log.\n",
        markdown_cell(&junit.display().to_string())
    )
}

fn write_report(path: &Path, markdown: &str) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create stress report directory {}", parent.display()))?;
    }
    fs::write(path, markdown).with_context(|| format!("write stress report {}", path.display()))
}

fn render(cases: &[CaseTiming], expected_count: usize) -> String {
    let mut tests = BTreeMap::<(String, String), TestStats>::new();
    let mut observed_iterations = BTreeSet::new();
    let mut failed_attempts = 0;
    for case in cases {
        let stats = tests
            .entry((case.suite.clone(), case.name.clone()))
            .or_default();
        stats.attempts += 1;
        stats.max_secs = stats.max_secs.max(case.secs);
        if let Some(iteration) = case.iteration {
            observed_iterations.insert(iteration);
            stats.observed_iterations.insert(iteration);
            if case.failed {
                stats.failed_iterations.insert(iteration);
            }
        }
        if case.failed {
            stats.failures += 1;
            failed_attempts += 1;
        }
    }

    let expected_iterations = (0..expected_count).collect::<BTreeSet<_>>();
    let observed_count = observed_iterations.len();
    let complete = !cases.is_empty()
        && observed_iterations == expected_iterations
        && tests
            .values()
            .all(|stats| stats.observed_iterations == expected_iterations);
    let result = if !complete {
        "INCOMPLETE"
    } else if failed_attempts > 0 {
        "FAILED"
    } else {
        "PASSED"
    };

    let mut out = String::from("# Stress evidence\n\n");
    let _ = writeln!(out, "- Result: **{result}**");
    let _ = writeln!(out, "- Requested iterations: `{expected_count}`");
    let _ = writeln!(out, "- Observed iterations: `{observed_count}`");
    let _ = writeln!(out, "- Tests observed: `{}`", tests.len());
    let _ = writeln!(out, "- Test attempts: `{}`", cases.len());
    let _ = writeln!(out, "- Failed attempts: `{failed_attempts}`");

    let mut failures = tests
        .into_iter()
        .filter(|(_, stats)| stats.failures > 0)
        .collect::<Vec<_>>();
    failures.sort_by(|left, right| {
        Reverse(left.1.failures)
            .cmp(&Reverse(right.1.failures))
            .then_with(|| left.0.cmp(&right.0))
    });
    if failures.is_empty() {
        out.push_str("\nNo failed attempts were recorded.\n");
        return out;
    }

    let _ = writeln!(
        out,
        "\n## Failed tests\n\n| test | failed / attempts | rate | failed iterations (zero-based) | max |\n|---|---:|---:|---|---:|"
    );
    for ((suite, name), stats) in failures.iter().take(MAX_FAILURE_ROWS) {
        let rate = rate_percent(stats.failures, stats.attempts);
        let iterations = render_iterations(&stats.failed_iterations);
        let id = markdown_cell(&format!("{suite} {name}"));
        let _ = writeln!(
            out,
            "| `{id}` | {} / {} | {rate} | {iterations} | {:.0} ms |",
            stats.failures,
            stats.attempts,
            stats.max_secs * 1000.0,
        );
    }
    if failures.len() > MAX_FAILURE_ROWS {
        let _ = writeln!(
            out,
            "\nShowing the first {MAX_FAILURE_ROWS} of {} failed tests. The JUnit artifact is exhaustive.",
            failures.len()
        );
    }
    out
}

fn rate_percent(failures: usize, attempts: usize) -> String {
    let hundredths = failures.saturating_mul(PERCENT_HUNDREDTHS) / attempts;
    format!(
        "{}.{:02}%",
        hundredths / PERCENT_SCALE,
        hundredths % PERCENT_SCALE
    )
}

fn render_iterations(iterations: &BTreeSet<usize>) -> String {
    if iterations.is_empty() {
        return "unknown".to_owned();
    }
    let mut rendered = iterations
        .iter()
        .take(MAX_ITERATIONS_PER_TEST)
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    if iterations.len() > MAX_ITERATIONS_PER_TEST {
        let _ = write!(rendered, ", ... ({} total)", iterations.len());
    }
    rendered
}

fn markdown_cell(text: &str) -> String {
    text.replace('|', "\\|")
        .replace('\r', " ")
        .replace('\n', " ")
        .replace('`', "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(name: &str, iteration: usize, failed: bool, secs: f64) -> CaseTiming {
        CaseTiming {
            name: name.to_owned(),
            suite: "demo::tests".to_owned(),
            iteration: Some(iteration),
            failed,
            secs,
        }
    }

    #[test]
    fn reports_failure_rate_and_exact_iterations() {
        let cases = vec![
            case("seek", 0, false, 0.1),
            case("seek", 1, true, 0.25),
            case("seek", 2, false, 0.2),
            case("other", 0, false, 0.01),
            case("other", 1, false, 0.01),
            case("other", 2, false, 0.01),
        ];

        let report = render(&cases, 3);

        assert!(report.contains("Result: **FAILED**"), "{report}");
        assert!(report.contains("1 / 3"), "{report}");
        assert!(report.contains("33.33%"), "{report}");
        assert!(report.contains("| 1 | 250 ms |"), "{report}");
        assert!(!report.contains("demo::tests other"), "{report}");
    }

    #[test]
    fn distinguishes_a_green_partial_report_from_a_complete_run() {
        let cases = vec![case("seek", 0, false, 0.1)];

        let partial = render(&cases, 2);
        let complete = render(&cases, 1);

        assert!(partial.contains("Result: **INCOMPLETE**"), "{partial}");
        assert!(complete.contains("Result: **PASSED**"), "{complete}");
    }

    #[test]
    fn a_failure_before_all_iterations_is_still_incomplete() {
        let cases = vec![case("seek", 0, true, 0.1)];

        let report = render(&cases, 2);

        assert!(report.contains("Result: **INCOMPLETE**"), "{report}");
        assert!(report.contains("Failed attempts: `1`"), "{report}");
    }

    #[test]
    fn duplicate_iterations_do_not_hide_a_gap() {
        let cases = vec![
            case("seek", 0, false, 0.1),
            case("seek", 0, false, 0.1),
            case("seek", 2, false, 0.1),
        ];

        let report = render(&cases, 3);

        assert!(report.contains("Result: **INCOMPLETE**"), "{report}");
        assert!(report.contains("Observed iterations: `2`"), "{report}");
    }

    #[test]
    fn missing_junit_is_explicitly_incomplete() {
        let report = render_missing(50, Path::new("target/nextest/stress/junit.xml"));

        assert!(report.contains("Result: **NO JUNIT**"), "{report}");
        assert!(report.contains("Requested iterations: `50`"), "{report}");
        assert!(report.contains("primary step log"), "{report}");
    }
}
