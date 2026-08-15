//! Builds a bounded Markdown summary from a nextest stress `JUnit` report.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde::Deserialize;

use crate::{
    common::project::StressEvidenceConfig,
    junit::{CaseTiming, parse_junit_report},
    verdict::NotClean,
};

mod evidence;

const MAX_FAILURE_ROWS: usize = 100;
const MAX_PROBLEM_ROWS: usize = 100;
const MAX_ITERATIONS_PER_TEST: usize = 20;
const MAX_CELL_CHARS: usize = 240;
const PERCENT_SCALE: usize = 100;
const PERCENT_HUNDREDTHS: usize = PERCENT_SCALE * PERCENT_SCALE;
const MAX_INVENTORY_CASES: usize = 100_000;
pub(crate) const MAX_INVENTORY_BYTES: u64 = 64 * 1_024 * 1_024;
pub(crate) const MAX_JUNIT_BYTES: u64 = 512 * 1_024 * 1_024;

#[derive(Debug, Args)]
pub(crate) struct StressReportArgs {
    /// `JUnit` emitted by the nextest stress profile.
    #[arg(long)]
    junit: PathBuf,
    /// Machine-readable output from `cargo nextest list` for the same selection.
    #[arg(long)]
    inventory: PathBuf,
    /// Optional line evidence whose records carry nextest attempt identifiers.
    #[arg(long)]
    line_log: Option<PathBuf>,
    /// Optional directory containing structured attempt envelopes.
    #[arg(long)]
    envelope_dir: Option<PathBuf>,
    /// Optional one-second Linux host and cgroup pressure samples.
    #[arg(long)]
    pressure_log: Option<PathBuf>,
    /// Markdown summary destination.
    #[arg(long)]
    output: PathBuf,
    /// Number of stress iterations requested from nextest.
    #[arg(long)]
    expected_count: usize,
    /// Explain an absent `JUnit` as fallout from the primary nextest step.
    #[arg(long)]
    allow_missing: bool,
    #[arg(skip)]
    evidence: StressEvidenceConfig,
}

impl StressReportArgs {
    #[must_use]
    pub(crate) fn new(
        junit: PathBuf,
        inventory: PathBuf,
        output: PathBuf,
        expected_count: usize,
    ) -> Self {
        Self {
            junit,
            inventory,
            line_log: None,
            envelope_dir: None,
            pressure_log: None,
            output,
            expected_count,
            allow_missing: false,
            evidence: StressEvidenceConfig::default(),
        }
    }

    #[must_use]
    pub(crate) fn with_allow_missing(mut self, allow_missing: bool) -> Self {
        self.allow_missing = allow_missing;
        self
    }

    #[must_use]
    pub(crate) fn with_pressure(mut self, path: PathBuf) -> Self {
        self.pressure_log = Some(path);
        self
    }

    #[must_use]
    pub(crate) fn with_optional_envelopes(mut self, path: Option<PathBuf>) -> Self {
        self.envelope_dir = path;
        self
    }

    #[must_use]
    pub(crate) fn with_optional_lines(mut self, path: Option<PathBuf>) -> Self {
        self.line_log = path;
        self
    }

    #[must_use]
    pub(crate) fn with_evidence(mut self, evidence: StressEvidenceConfig) -> Self {
        self.evidence = evidence;
        self
    }
}

#[derive(Debug, Default)]
struct TestStats {
    max_secs: f64,
    observed_iterations: BTreeSet<usize>,
    failed_iterations: BTreeSet<usize>,
}

#[derive(Debug)]
struct RenderedReport {
    markdown: String,
    complete: bool,
    /// What each test did in this lane, kept so lanes can be put side by side.
    rates: BTreeMap<TestId, LaneRate>,
}

#[derive(Debug, Default)]
struct EvidenceProblems {
    rows: Vec<String>,
    total: usize,
    invalid: bool,
}

type TestId = (String, String);

#[derive(Debug, Deserialize)]
struct Inventory {
    #[serde(rename = "rust-suites")]
    rust_suites: BTreeMap<String, InventorySuite>,
}

#[derive(Debug, Deserialize)]
struct InventorySuite {
    #[serde(rename = "binary-id")]
    binary_id: String,
    status: String,
    testcases: BTreeMap<String, InventoryCase>,
}

#[derive(Debug, Deserialize)]
struct InventoryCase {
    ignored: bool,
    #[serde(rename = "filter-match")]
    filter_match: InventoryMatch,
}

#[derive(Debug, Deserialize)]
struct InventoryMatch {
    status: String,
}

impl EvidenceProblems {
    fn add(&mut self, problem: String, invalid: bool) {
        self.total = self.total.saturating_add(1);
        self.invalid |= invalid;
        if self.rows.len() < MAX_PROBLEM_ROWS {
            self.rows.push(problem);
        }
    }
}

/// Summarizes one nextest stress run.
///
/// # Errors
///
/// Returns an error when the evidence is absent, incomplete, invalid, or
/// contains a failed attempt, the expected count is zero, or the output cannot
/// be written.
pub(crate) fn run(args: &StressReportArgs) -> Result<()> {
    let lane = lane_report(args)?;
    write_report(&args.output, &lane.markdown)?;
    lane.verdict
}

/// One lane's evidence: what it reads out of the artifact, and the verdict that
/// reading carries.
///
/// Rendering is separated from writing so that a campaign of several lanes can
/// put them all in one document. A file per lane would leave the question the
/// campaign exists to answer — which lane does this flake belong to — spread
/// across two documents for the reader to join by hand.
pub(crate) struct LaneReport {
    pub(crate) markdown: String,
    pub(crate) rates: BTreeMap<TestId, LaneRate>,
    pub(crate) verdict: Result<()>,
}

/// How often one test failed in one lane.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct LaneRate {
    pub(crate) failed: usize,
    pub(crate) attempts: usize,
}

pub(crate) fn lane_report(args: &StressReportArgs) -> Result<LaneReport> {
    validate_expected_count(args.expected_count)?;
    let unreadable = |markdown: String| {
        Ok(LaneReport {
            markdown,
            rates: BTreeMap::new(),
            verdict: Err(NotClean::reported("stress evidence")),
        })
    };
    let inventory = match read_inventory(&args.inventory) {
        Ok(inventory) => inventory,
        Err(error) => {
            let detail = format!("{error:#}");
            return unreadable(render_invalid_artifact(
                "INVALID INVENTORY",
                args.expected_count,
                &args.inventory,
                &detail,
            ));
        }
    };
    let xml = match read_bounded_utf8(&args.junit, MAX_JUNIT_BYTES, "stress JUnit") {
        Ok(xml) => xml,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            return unreadable(render_missing(
                args.expected_count,
                &args.junit,
                args.allow_missing,
            ));
        }
        Err(error) => {
            let detail = format!("{error:#}");
            return unreadable(render_invalid_artifact(
                "INVALID JUNIT",
                args.expected_count,
                &args.junit,
                &detail,
            ));
        }
    };
    let junit = match parse_junit_report(&xml) {
        Ok(junit) => junit,
        Err(error) => {
            let detail = format!("{error:#}");
            return unreadable(render_invalid_artifact(
                "INVALID JUNIT",
                args.expected_count,
                &args.junit,
                &detail,
            ));
        }
    };
    let has_failures = junit.cases.iter().any(|case| case.failed);
    if let Err(error) = validate_correlation_metadata(&junit) {
        return unreadable(render_invalid_artifact(
            "INVALID JUNIT",
            args.expected_count,
            &args.junit,
            &error,
        ));
    }
    let mut report = render(
        &junit.cases,
        &inventory,
        args.expected_count,
        junit.run_id.as_deref(),
        junit.timestamp.as_deref(),
    );
    let correlated_complete = evidence::append_correlated_evidence(
        &mut report.markdown,
        &junit.cases,
        junit.run_id.as_deref(),
        args,
    );
    if !correlated_complete {
        report.complete = false;
        mark_incomplete(&mut report.markdown);
    }
    let verdict = if report.complete && !has_failures {
        Ok(())
    } else {
        Err(NotClean::reported("stress evidence"))
    };
    Ok(LaneReport {
        markdown: report.markdown,
        rates: report.rates,
        verdict,
    })
}

/// Check the inventory-by-iteration contract before the campaign records its
/// primary exit status. The full reporter later writes the actionable detail;
/// this narrow verdict prevents a missing or partial `JUnit` from being
/// mistaken for a successful nextest stress run.
///
/// # Errors
///
/// Returns a concise stress verdict when either artifact is unavailable or
/// invalid, any selected attempt is absent, or any recorded attempt failed.
pub(crate) fn validate_primary_evidence(
    inventory_path: &Path,
    junit_path: &Path,
    expected_count: usize,
) -> Result<()> {
    validate_expected_count(expected_count)?;
    let inventory =
        read_inventory(inventory_path).map_err(|_| NotClean::reported("stress run evidence"))?;
    let xml = read_bounded_utf8(junit_path, MAX_JUNIT_BYTES, "stress JUnit")
        .map_err(|_| NotClean::reported("stress run evidence"))?;
    let junit = parse_junit_report(&xml).map_err(|_| NotClean::reported("stress run evidence"))?;
    validate_correlation_metadata(&junit).map_err(|_| NotClean::reported("stress run evidence"))?;
    let report = render(
        &junit.cases,
        &inventory,
        expected_count,
        junit.run_id.as_deref(),
        junit.timestamp.as_deref(),
    );
    if report.complete && !junit.cases.iter().any(|case| case.failed) {
        Ok(())
    } else {
        Err(NotClean::reported("stress run evidence"))
    }
}

fn validate_correlation_metadata(junit: &crate::junit::JunitReport) -> Result<(), String> {
    if junit.run_id.as_deref().is_none_or(str::is_empty) {
        return Err("nextest JUnit root uuid is missing".to_owned());
    }
    let Some(timestamp) = &junit.timestamp else {
        return Err("nextest JUnit root timestamp is missing".to_owned());
    };
    if evidence::parse_timestamp_ms(timestamp).is_none() {
        return Err("nextest JUnit root timestamp is malformed".to_owned());
    }
    for case in &junit.cases {
        let Some(timestamp) = case.timestamp.as_deref() else {
            return Err(format!(
                "testcase timestamp is missing on {} {}",
                case.suite, case.name
            ));
        };
        if evidence::parse_timestamp_ms(timestamp).is_none() {
            return Err(format!(
                "testcase timestamp is malformed on {} {}",
                case.suite, case.name
            ));
        }
    }
    Ok(())
}

fn render_missing(expected_count: usize, junit: &Path, allow_missing: bool) -> String {
    let explanation = if allow_missing {
        "Nextest did not reach the point where it could write per-iteration evidence. Inspect the primary step log."
    } else {
        "The required per-iteration evidence was not produced. Inspect the command and input path."
    };
    format!(
        "# Stress evidence\n\n- Result: **NO JUNIT**\n- Requested iterations: `{expected_count}`\n- JUnit path: `{}`\n\n{explanation}\n",
        markdown_cell(&junit.display().to_string()),
    )
}

fn render_invalid_artifact(
    result: &str,
    expected_count: usize,
    artifact: &Path,
    detail: &str,
) -> String {
    format!(
        "# Stress evidence\n\n- Result: **{result}**\n- Requested iterations: `{expected_count}`\n- Evidence path: `{}`\n\n## Evidence problems\n\n- {}\n",
        markdown_cell(&artifact.display().to_string()),
        markdown_cell(detail),
    )
}

fn read_inventory(path: &Path) -> Result<BTreeSet<TestId>> {
    let json = read_bounded_utf8(path, MAX_INVENTORY_BYTES, "stress inventory")?;
    parse_inventory(&json)
}

pub(crate) fn read_bounded_utf8(path: &Path, max_bytes: u64, artifact: &str) -> Result<String> {
    let file =
        File::open(path).with_context(|| format!("open {artifact} at {}", path.display()))?;
    let length = file
        .metadata()
        .with_context(|| format!("read {artifact} metadata at {}", path.display()))?
        .len();
    if length > max_bytes {
        bail!(
            "{artifact} exceeds the deterministic limit of {max_bytes} bytes (observed {length}); the raw artifact was left untouched"
        );
    }
    let capacity = usize::try_from(length).context("artifact length does not fit in memory")?;
    let max_capacity =
        usize::try_from(max_bytes).context("artifact byte limit does not fit in memory")?;
    let read_capacity = capacity
        .checked_add(1)
        .context("artifact read capacity overflow")?;
    let mut bytes = Vec::with_capacity(read_capacity);
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {artifact} at {}", path.display()))?;
    if bytes.len() > max_capacity {
        bail!(
            "{artifact} exceeds the deterministic limit of {max_bytes} bytes; the raw artifact was left untouched"
        );
    }
    String::from_utf8(bytes).with_context(|| format!("{artifact} is not UTF-8"))
}

pub(crate) fn validate_inventory(json: &str) -> Result<()> {
    parse_inventory(json).map(|_| ())
}

fn parse_inventory(json: &str) -> Result<BTreeSet<TestId>> {
    let inventory: Inventory = serde_json::from_str(json).context("parse stress inventory JSON")?;
    if inventory.rust_suites.is_empty() {
        bail!("stress inventory contains no Rust test suites");
    }
    let mut tests = BTreeSet::new();
    let mut inventory_cases = 0usize;
    for (suite, inventory) in inventory.rust_suites {
        if suite.trim().is_empty() || inventory.binary_id.trim().is_empty() {
            bail!("stress inventory contains an empty suite identity");
        }
        if suite != inventory.binary_id {
            bail!("stress inventory suite key does not match its binary-id");
        }
        match inventory.status.as_str() {
            "listed" => {}
            // A target the project's `default-filter` excludes is inventoried
            // with this status and none of its cases. It owes the campaign
            // nothing, and reading the exclusion as a malformed inventory is
            // how a campaign ends before its first test. The suite is dropped
            // whole rather than filtered case by case, so a future nextest
            // that does list them still cannot contribute any.
            "skipped-default-filter" => continue,
            status => bail!("stress inventory suite `{suite}` has unsupported status `{status}`"),
        }
        for (name, case) in inventory.testcases {
            inventory_cases = inventory_cases.saturating_add(1);
            validate_inventory_case_count(inventory_cases)?;
            if name.trim().is_empty() {
                bail!("stress inventory contains an empty test name");
            }
            match case.filter_match.status.as_str() {
                "matches" if !case.ignored => {
                    tests.insert((suite.clone(), name));
                }
                "matches" | "mismatch" => {}
                status => bail!("stress inventory contains unknown filter status `{status}`"),
            }
        }
    }
    if tests.is_empty() {
        bail!("stress inventory contains no runnable selected tests");
    }
    Ok(tests)
}

fn validate_inventory_case_count(count: usize) -> Result<()> {
    if count > MAX_INVENTORY_CASES {
        bail!(
            "stress inventory exceeds the deterministic limit of {MAX_INVENTORY_CASES} testcases"
        );
    }
    Ok(())
}

fn validate_expected_count(expected_count: usize) -> Result<()> {
    if expected_count == 0 {
        bail!("expected-count must be greater than zero");
    }
    Ok(())
}

/// The lanes of one campaign, side by side, ordered by how much they disagree.
///
/// Disagreement is what the table is for. A test that fails at the same rate on
/// both clocks is a flake that owes nothing to either of them; a test that
/// fails on one and not the other is the campaign's whole point, and it must
/// not be buried under a hundred rows of the first kind.
///
/// A test present in one lane and absent from the other is reported as absent
/// rather than as zero: the lanes do not select the same tests, because targets
/// behind a feature exist in one and not the other, and calling that a rate of
/// zero would invent a passing result for a test that never ran.
pub(crate) fn render_lane_comparison(
    lanes: &[(String, BTreeMap<TestId, LaneRate>)],
    requested: usize,
) -> String {
    let mut out = String::from("# Stress campaign\n");
    let _ = writeln!(out, "\n- Lanes requested: `{requested}`");
    let _ = writeln!(out, "- Lanes with trustworthy evidence: `{}`", lanes.len());
    if lanes.len() < 2 {
        out.push_str(
            "\nA comparison needs two verified lanes. The lane sections below stand on their own.\n",
        );
        return out;
    }

    let mut rows = BTreeMap::<TestId, Vec<Option<LaneRate>>>::new();
    for (index, (_, rates)) in lanes.iter().enumerate() {
        for (id, rate) in rates {
            rows.entry(id.clone())
                .or_insert_with(|| vec![None; lanes.len()])[index] = Some(*rate);
        }
    }
    let mut ranked = rows
        .into_iter()
        .filter(|(_, cells)| cells.iter().flatten().any(|rate| rate.failed > 0))
        .map(|(id, cells)| {
            let spread = disagreement(&cells);
            (spread, id, cells)
        })
        .collect::<Vec<_>>();
    if ranked.is_empty() {
        out.push_str("\nNo test failed in any lane.\n");
        return out;
    }
    ranked.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.cmp(&right.1))
    });

    out.push_str("\n## Failure rate by lane\n\n| test |");
    for (name, _) in lanes {
        let _ = write!(out, " {} |", markdown_cell(name));
    }
    out.push_str("\n|---|");
    for _ in lanes {
        out.push_str("---:|");
    }
    out.push('\n');
    for (_, (suite, name), cells) in ranked.iter().take(MAX_FAILURE_ROWS) {
        let _ = write!(out, "| `{}` |", markdown_cell(&format!("{suite} {name}")));
        for cell in cells {
            match cell {
                Some(rate) => {
                    let _ = write!(
                        out,
                        " {} ({}/{}) |",
                        rate_percent(rate.failed, rate.attempts),
                        rate.failed,
                        rate.attempts
                    );
                }
                None => out.push_str(" not selected |"),
            }
        }
        out.push('\n');
    }
    if ranked.len() > MAX_FAILURE_ROWS {
        let _ = writeln!(
            out,
            "\nShowing the first {MAX_FAILURE_ROWS} of {} tests that failed somewhere.",
            ranked.len()
        );
    }
    out
}

/// How far apart the lanes are on one test, as a fraction.
///
/// A test selected by only some of the lanes is maximally interesting: the
/// lanes cannot even be compared on it, and the reader should see it first.
fn disagreement(cells: &[Option<LaneRate>]) -> f64 {
    if cells.iter().any(Option::is_none) {
        return f64::INFINITY;
    }
    let rates = cells
        .iter()
        .flatten()
        .map(|rate| {
            if rate.attempts == 0 {
                0.0
            } else {
                f64::from(u32::try_from(rate.failed).unwrap_or(u32::MAX))
                    / f64::from(u32::try_from(rate.attempts).unwrap_or(u32::MAX))
            }
        })
        .collect::<Vec<_>>();
    let high = rates.iter().copied().fold(f64::MIN, f64::max);
    let low = rates.iter().copied().fold(f64::MAX, f64::min);
    high - low
}

pub(crate) fn write_report(path: &Path, markdown: &str) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create stress report directory {}", parent.display()))?;
    }
    fs::write(path, markdown).with_context(|| format!("write stress report {}", path.display()))
}

fn mark_incomplete(markdown: &mut String) {
    for result in ["PASSED", "FAILED"] {
        let marker = format!("- Result: **{result}**");
        if let Some(index) = markdown.find(&marker) {
            markdown.replace_range(index..index + marker.len(), "- Result: **INCOMPLETE**");
            return;
        }
    }
}

fn render(
    cases: &[CaseTiming],
    inventory: &BTreeSet<TestId>,
    expected_count: usize,
    run_id: Option<&str>,
    timestamp: Option<&str>,
) -> RenderedReport {
    let mut tests = BTreeMap::<(String, String), TestStats>::new();
    let mut observed_iterations = BTreeSet::new();
    let mut unique_cases = BTreeSet::new();
    let mut problems = EvidenceProblems::default();
    if cases.is_empty() {
        problems.add("the JUnit report contains no testcases".to_owned(), false);
    }
    for case in cases {
        if case.suite.trim().is_empty() || case.name.trim().is_empty() {
            problems.add("a testcase has an empty suite or name".to_owned(), true);
            continue;
        }
        let id = (case.suite.clone(), case.name.clone());
        if !inventory.contains(&id) {
            problems.add(
                format!("JUnit contains unselected test `{}`", test_id(case)),
                true,
            );
            continue;
        }
        let Some(iteration) = case.iteration else {
            problems.add(
                format!("test `{}` has no stress iteration", test_id(case)),
                true,
            );
            continue;
        };
        if iteration >= expected_count {
            problems.add(
                format!(
                    "test `{}` has out-of-range iteration {iteration}",
                    test_id(case)
                ),
                true,
            );
            continue;
        }
        if !unique_cases.insert((case.suite.clone(), case.name.clone(), iteration)) {
            problems.add(
                format!(
                    "test `{}` has duplicate iteration {iteration}",
                    test_id(case)
                ),
                true,
            );
            continue;
        }
        let stats = tests
            .entry((case.suite.clone(), case.name.clone()))
            .or_default();
        stats.max_secs = stats.max_secs.max(case.secs);
        observed_iterations.insert(iteration);
        stats.observed_iterations.insert(iteration);
        if case.failed {
            stats.failed_iterations.insert(iteration);
        }
    }

    add_coverage_problem(
        &mut problems,
        "the report",
        &observed_iterations,
        expected_count,
    );
    for (suite, name) in inventory {
        if !tests.contains_key(&(suite.clone(), name.clone())) {
            let id = markdown_cell(&format!("{suite} {name}"));
            problems.add(
                format!("selected test `{id}` is absent from the JUnit report"),
                false,
            );
        }
    }
    for ((suite, name), stats) in &tests {
        let id = markdown_cell(&format!("{suite} {name}"));
        let subject = format!("test `{id}`");
        add_coverage_problem(
            &mut problems,
            &subject,
            &stats.observed_iterations,
            expected_count,
        );
    }

    let rates = tests
        .iter()
        .map(|(id, stats)| {
            (
                id.clone(),
                LaneRate {
                    failed: stats.failed_iterations.len(),
                    attempts: stats.observed_iterations.len(),
                },
            )
        })
        .collect();
    let observed_count = observed_iterations.len();
    let complete = problems.total == 0;
    let failed_attempts = tests
        .values()
        .map(|stats| stats.failed_iterations.len())
        .sum::<usize>();
    let attempts = tests
        .values()
        .map(|stats| stats.observed_iterations.len())
        .sum::<usize>();
    let result = if problems.invalid {
        "INVALID JUNIT"
    } else if !complete {
        "INCOMPLETE"
    } else if failed_attempts > 0 {
        "FAILED"
    } else {
        "PASSED"
    };

    let mut out = String::from("# Stress evidence\n\n");
    let _ = writeln!(out, "- Result: **{result}**");
    if let Some(run_id) = run_id {
        let _ = writeln!(out, "- Nextest run ID: `{}`", markdown_cell(run_id));
    }
    if let Some(timestamp) = timestamp {
        let _ = writeln!(out, "- Campaign started: `{}`", markdown_cell(timestamp));
    }
    let _ = writeln!(out, "- Requested iterations: `{expected_count}`");
    let _ = writeln!(out, "- Observed iterations: `{observed_count}`");
    let _ = writeln!(out, "- Tests observed: `{}`", tests.len());
    let _ = writeln!(out, "- Tests selected: `{}`", inventory.len());
    let _ = writeln!(out, "- Testcases read: `{}`", cases.len());
    let _ = writeln!(out, "- Unique test iterations: `{attempts}`");
    let _ = writeln!(out, "- Failed attempts: `{failed_attempts}`");

    if problems.total > 0 {
        out.push_str("\n## Evidence problems\n");
        for problem in &problems.rows {
            let _ = writeln!(out, "\n- {problem}");
        }
        if problems.total > problems.rows.len() {
            let _ = writeln!(
                out,
                "\n- ... and {} more problems; inspect the JUnit artifact",
                problems.total - problems.rows.len()
            );
        }
    }

    render_failures(&mut out, tests);
    RenderedReport {
        markdown: out,
        complete,
        rates,
    }
}

fn render_failures(out: &mut String, tests: BTreeMap<TestId, TestStats>) {
    let mut failures = tests
        .into_iter()
        .filter(|(_, stats)| !stats.failed_iterations.is_empty())
        .collect::<Vec<_>>();
    failures.sort_by(|left, right| {
        Reverse(left.1.failed_iterations.len())
            .cmp(&Reverse(right.1.failed_iterations.len()))
            .then_with(|| left.0.cmp(&right.0))
    });
    if failures.is_empty() {
        out.push_str("\nNo failed attempts were recorded.\n");
        return;
    }

    let _ = writeln!(
        out,
        "\n## Failed tests\n\n| test | failed / attempts | rate | failed iterations (zero-based) | max |\n|---|---:|---:|---|---:|"
    );
    for ((suite, name), stats) in failures.iter().take(MAX_FAILURE_ROWS) {
        let failures = stats.failed_iterations.len();
        let attempts = stats.observed_iterations.len();
        let rate = rate_percent(failures, attempts);
        let iterations = render_iterations(&stats.failed_iterations);
        let id = markdown_cell(&format!("{suite} {name}"));
        let _ = writeln!(
            out,
            "| `{id}` | {} / {} | {rate} | {iterations} | {:.0} ms |",
            failures,
            attempts,
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
}

fn add_coverage_problem(
    problems: &mut EvidenceProblems,
    subject: &str,
    observed: &BTreeSet<usize>,
    expected_count: usize,
) {
    let in_range = observed.range(..expected_count).count();
    let missing_count = expected_count.saturating_sub(in_range);
    if missing_count == 0 {
        return;
    }
    let missing = (0..expected_count)
        .filter(|iteration| !observed.contains(iteration))
        .take(MAX_ITERATIONS_PER_TEST)
        .collect::<BTreeSet<_>>();
    let mut detail = render_iterations(&missing);
    if missing_count > MAX_ITERATIONS_PER_TEST {
        detail = format!("{detail}, ... ({missing_count} total)");
    }
    problems.add(
        format!("{subject} is missing requested iterations: {detail}"),
        false,
    );
}

fn test_id(case: &CaseTiming) -> String {
    markdown_cell(&format!("{} {}", case.suite, case.name))
}

fn rate_percent(failures: usize, attempts: usize) -> String {
    if attempts == 0 {
        return "0.00%".to_owned();
    }
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
    let sanitized = text
        .replace('|', "\\|")
        .replace(['\r', '\n'], " ")
        .replace('`', "'");
    let mut chars = sanitized.chars();
    let mut bounded = chars.by_ref().take(MAX_CELL_CHARS).collect::<String>();
    if chars.next().is_some() {
        bounded.push_str("...");
    }
    bounded
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
            timestamp: None,
            output: String::new(),
        }
    }

    fn inventory(names: &[&str]) -> BTreeSet<TestId> {
        names
            .iter()
            .map(|name| ("demo::tests".to_owned(), (*name).to_owned()))
            .collect()
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

        let report = render(&cases, &inventory(&["seek", "other"]), 3, None, None);
        let markdown = &report.markdown;

        assert!(report.complete, "{markdown}");
        assert!(markdown.contains("Result: **FAILED**"), "{markdown}");
        assert!(markdown.contains("1 / 3"), "{markdown}");
        assert!(markdown.contains("33.33%"), "{markdown}");
        assert!(markdown.contains("| 1 | 250 ms |"), "{markdown}");
        assert!(!markdown.contains("demo::tests other"), "{markdown}");
    }

    #[test]
    fn distinguishes_a_green_partial_report_from_a_complete_run() {
        let cases = vec![case("seek", 0, false, 0.1)];

        let inventory = inventory(&["seek"]);
        let partial = render(&cases, &inventory, 2, None, None);
        let complete = render(&cases, &inventory, 1, None, None);

        assert!(!partial.complete, "{}", partial.markdown);
        assert!(
            partial.markdown.contains("Result: **INCOMPLETE**"),
            "{}",
            partial.markdown
        );
        assert!(complete.complete, "{}", complete.markdown);
        assert!(
            complete.markdown.contains("Result: **PASSED**"),
            "{}",
            complete.markdown
        );
    }

    #[test]
    fn a_failure_before_all_iterations_is_still_incomplete() {
        let cases = vec![case("seek", 0, true, 0.1)];

        let report = render(&cases, &inventory(&["seek"]), 2, None, None);

        assert!(!report.complete, "{}", report.markdown);
        assert!(
            report.markdown.contains("Result: **INCOMPLETE**"),
            "{}",
            report.markdown
        );
        assert!(
            report.markdown.contains("Failed attempts: `1`"),
            "{}",
            report.markdown
        );
    }

    #[test]
    fn duplicate_iterations_do_not_hide_a_gap() {
        let cases = vec![
            case("seek", 0, false, 0.1),
            case("seek", 0, false, 0.1),
            case("seek", 2, false, 0.1),
        ];

        let report = render(&cases, &inventory(&["seek"]), 3, None, None);

        assert!(!report.complete, "{}", report.markdown);
        assert!(
            report.markdown.contains("Result: **INVALID JUNIT**"),
            "{}",
            report.markdown
        );
        assert!(
            report.markdown.contains("duplicate iteration 0"),
            "{}",
            report.markdown
        );
        assert!(
            report.markdown.contains("Observed iterations: `2`"),
            "{}",
            report.markdown
        );
    }

    #[test]
    fn missing_junit_is_explicitly_incomplete() {
        let report = render_missing(50, Path::new("target/nextest/stress/junit.xml"), true);

        assert!(report.contains("Result: **NO JUNIT**"), "{report}");
        assert!(report.contains("Requested iterations: `50`"), "{report}");
        assert!(report.contains("primary step log"), "{report}");
    }

    #[test]
    fn invalid_entries_do_not_inflate_failure_rates() {
        let mut unindexed = case("seek", 0, false, 0.4);
        unindexed.iteration = None;
        let cases = vec![
            case("seek", 0, true, 0.1),
            case("seek", 0, false, 0.2),
            case("seek", 2, true, 0.3),
            unindexed,
        ];

        let report = render(&cases, &inventory(&["seek"]), 1, None, None);
        let markdown = &report.markdown;

        assert!(!report.complete, "{markdown}");
        assert!(markdown.contains("duplicate iteration 0"), "{markdown}");
        assert!(markdown.contains("out-of-range iteration 2"), "{markdown}");
        assert!(markdown.contains("has no stress iteration"), "{markdown}");
        assert!(markdown.contains("Observed iterations: `1`"), "{markdown}");
        assert!(markdown.contains("1 / 1"), "{markdown}");
        assert!(markdown.contains("100.00%"), "{markdown}");
        assert!(markdown.contains("100 ms"), "{markdown}");
        assert!(!markdown.contains("2 / 2"), "{markdown}");
        assert!(!markdown.contains("200 ms"), "{markdown}");
        assert!(!markdown.contains("300 ms"), "{markdown}");
        assert!(!markdown.contains("400 ms"), "{markdown}");
    }

    #[test]
    fn invalid_evidence_details_stay_bounded() {
        let too_wide = "x".repeat(MAX_CELL_CHARS + 1);
        let cases = (0..MAX_PROBLEM_ROWS + 5)
            .map(|index| {
                let mut timing = case(&format!("case-{index}"), 0, false, 0.1);
                timing.suite.clone_from(&too_wide);
                timing.iteration = None;
                timing
            })
            .collect::<Vec<_>>();

        let names = (0..MAX_PROBLEM_ROWS + 5)
            .map(|index| format!("case-{index}"))
            .collect::<Vec<_>>();
        let selected = names
            .iter()
            .map(|name| (too_wide.clone(), name.clone()))
            .collect();
        let report = render(&cases, &selected, 1, None, None);
        let markdown = &report.markdown;

        assert_eq!(
            markdown.matches("has no stress iteration").count(),
            MAX_PROBLEM_ROWS,
            "{markdown}"
        );
        assert!(markdown.contains("more problems"), "{markdown}");
        assert!(!markdown.contains(too_wide.as_str()), "{markdown}");
    }

    #[test]
    fn incomplete_inputs_write_reports_and_return_a_verdict() {
        const PARTIAL: &str = r#"<testsuites uuid="run" timestamp="2026-08-13T12:00:00Z">
  <testsuite name="demo::tests@stress-0">
    <testcase name="seek" classname="demo::tests" time="0.1" timestamp="2026-08-13T12:00:00Z"/>
  </testsuite>
</testsuites>"#;
        const MISMATCHED: &str = r#"<testsuites uuid="run" timestamp="2026-08-13T12:00:00Z">
  <testsuite name="other::tests@stress-0">
    <testcase name="seek" classname="demo::tests" time="0.1" timestamp="2026-08-13T12:00:00Z"/>
  </testsuite>
</testsuites>"#;
        let temp = tempfile::tempdir().expect("tempdir");
        let inventory = temp.path().join("inventory.json");
        fs::write(
            &inventory,
            r#"{
  "rust-suites": {
    "demo::tests": {
      "binary-id": "demo::tests",
      "status": "listed",
      "testcases": {
        "seek": {"ignored": false, "filter-match": {"status": "matches"}}
      }
    }
  }
}"#,
        )
        .expect("write inventory");
        let scenarios = [
            ("missing", None, "Result: **NO JUNIT**"),
            (
                "empty",
                Some("<testsuites uuid=\"run\"/>"),
                "Result: **INVALID JUNIT**",
            ),
            ("partial", Some(PARTIAL), "Result: **INCOMPLETE**"),
            ("mismatched", Some(MISMATCHED), "Result: **INVALID JUNIT**"),
        ];

        for (name, xml, marker) in scenarios {
            let junit = temp.path().join(format!("{name}.xml"));
            let output = temp.path().join(format!("{name}.md"));
            if let Some(xml) = xml {
                fs::write(&junit, xml).expect("write fixture");
            }
            let args = StressReportArgs {
                junit,
                inventory: inventory.clone(),
                line_log: None,
                envelope_dir: None,
                pressure_log: None,
                output: output.clone(),
                expected_count: 2,
                allow_missing: true,
                evidence: StressEvidenceConfig::default(),
            };

            let error = run(&args).expect_err("incomplete evidence must fail closed");
            let markdown = fs::read_to_string(output).expect("read report");

            assert!(error.downcast_ref::<NotClean>().is_some(), "{error:?}");
            assert!(markdown.contains(marker), "{markdown}");
        }
    }

    #[test]
    fn complete_failed_input_writes_report_and_returns_a_verdict() {
        const FAILED: &str = r#"<testsuites uuid="run" timestamp="2026-08-13T12:00:00Z">
  <testsuite name="demo::tests@stress-0">
    <testcase name="seek" classname="demo::tests" time="0.1" timestamp="2026-08-13T12:00:00Z">
      <failure type="test failure">boom</failure>
    </testcase>
  </testsuite>
  <testsuite name="demo::tests@stress-1">
    <testcase name="seek" classname="demo::tests" time="0.1" timestamp="2026-08-13T12:00:01Z"/>
  </testsuite>
</testsuites>"#;
        let temp = tempfile::tempdir().expect("tempdir");
        let inventory = temp.path().join("inventory.json");
        let junit = temp.path().join("junit.xml");
        let output = temp.path().join("report.md");
        fs::write(
            &inventory,
            r#"{
  "rust-suites": {
    "demo::tests": {
      "binary-id": "demo::tests",
      "status": "listed",
      "testcases": {
        "seek": {"ignored": false, "filter-match": {"status": "matches"}}
      }
    }
  }
}"#,
        )
        .expect("write inventory");
        fs::write(&junit, FAILED).expect("write junit");
        let args = StressReportArgs {
            junit,
            inventory,
            line_log: None,
            envelope_dir: None,
            pressure_log: None,
            output: output.clone(),
            expected_count: 2,
            allow_missing: false,
            evidence: StressEvidenceConfig::default(),
        };

        let error = run(&args).expect_err("failed attempt must fail closed");
        let markdown = fs::read_to_string(output).expect("read report");

        assert!(error.downcast_ref::<NotClean>().is_some(), "{error:?}");
        assert!(markdown.contains("Result: **FAILED**"), "{markdown}");
        assert!(markdown.contains("Observed iterations: `2`"), "{markdown}");
        assert!(markdown.contains("Failed attempts: `1`"), "{markdown}");
    }

    #[test]
    fn selected_test_missing_from_junit_fails_closed() {
        let cases = vec![case("seek", 0, false, 0.1)];

        let report = render(&cases, &inventory(&["seek", "missing"]), 1, None, None);

        assert!(!report.complete, "{}", report.markdown);
        assert!(
            report
                .markdown
                .contains("selected test `demo::tests missing` is absent"),
            "{}",
            report.markdown
        );
    }

    #[test]
    fn inventory_parser_keeps_only_runnable_matches() {
        let json = r#"{
  "rust-suites": {
    "demo::tests": {
      "binary-id": "demo::tests",
      "status": "listed",
      "testcases": {
        "selected": {"ignored": false, "filter-match": {"status": "matches"}},
        "ignored": {"ignored": true, "filter-match": {"status": "matches"}},
        "filtered": {"ignored": false, "filter-match": {"status": "mismatch"}}
      }
    }
  }
}"#;

        assert_eq!(
            parse_inventory(json).expect("parse inventory"),
            inventory(&["selected"])
        );
    }

    /// A target the project's `default-filter` excludes is listed by nextest
    /// with this status and no testcases. Reading it as a malformed inventory
    /// stopped the campaign before it ran a single test.
    #[test]
    fn a_suite_skipped_by_the_default_filter_contributes_no_tests() {
        let json = r#"{
  "rust-suites": {
    "demo::tests": {
      "binary-id": "demo::tests",
      "status": "listed",
      "testcases": {
        "selected": {"ignored": false, "filter-match": {"status": "matches"}}
      }
    },
    "excluded::tests": {
      "binary-id": "excluded::tests",
      "status": "skipped-default-filter",
      "testcases": {
        "unreachable": {"ignored": false, "filter-match": {"status": "matches"}}
      }
    }
  }
}"#;

        assert_eq!(
            parse_inventory(json).expect("parse inventory"),
            inventory(&["selected"])
        );
    }

    #[test]
    fn a_suite_status_the_parser_does_not_know_is_rejected() {
        let json = r#"{
  "rust-suites": {
    "demo::tests": {
      "binary-id": "demo::tests",
      "status": "invented",
      "testcases": {}
    }
  }
}"#;

        assert!(
            parse_inventory(json)
                .expect_err("unknown suite status must fail")
                .to_string()
                .contains("unsupported status `invented`")
        );
    }

    #[test]
    fn an_inventory_of_nothing_but_skipped_suites_is_rejected() {
        let json = r#"{
  "rust-suites": {
    "excluded::tests": {
      "binary-id": "excluded::tests",
      "status": "skipped-default-filter",
      "testcases": {}
    }
  }
}"#;

        assert!(
            parse_inventory(json)
                .expect_err("an empty selection must fail")
                .to_string()
                .contains("no runnable selected tests")
        );
    }

    fn lane(name: &str, rows: &[(&str, usize, usize)]) -> (String, BTreeMap<TestId, LaneRate>) {
        (
            name.to_owned(),
            rows.iter()
                .map(|(test, failed, attempts)| {
                    (
                        ("demo::tests".to_owned(), (*test).to_owned()),
                        LaneRate {
                            failed: *failed,
                            attempts: *attempts,
                        },
                    )
                })
                .collect(),
        )
    }

    #[test]
    fn a_test_the_lanes_disagree_about_is_ranked_above_one_they_agree_on() {
        let table = render_lane_comparison(
            &[
                lane("on", &[("agreed", 5, 10), ("disputed", 0, 10)]),
                lane("off", &[("agreed", 5, 10), ("disputed", 9, 10)]),
            ],
            2,
        );

        let disputed = table.find("disputed").expect("disputed row");
        let agreed = table.find("agreed").expect("agreed row");
        assert!(disputed < agreed, "{table}");
    }

    #[test]
    fn a_test_absent_from_one_lane_is_reported_absent_rather_than_passing() {
        let table = render_lane_comparison(
            &[
                lane("on", &[("flash_only", 3, 10)]),
                lane("off", &[("shared", 1, 10)]),
            ],
            2,
        );

        assert!(table.contains("not selected"), "{table}");
    }

    #[test]
    fn a_campaign_with_one_trustworthy_lane_refuses_to_compare() {
        let table = render_lane_comparison(&[lane("on", &[("solo", 1, 10)])], 2);

        assert!(table.contains("needs two verified lanes"), "{table}");
    }

    #[test]
    fn positive_campaign_counts_and_inventory_case_limits_are_explicit() {
        for count in [1, 50, usize::MAX] {
            validate_expected_count(count).expect("valid expected count");
        }
        assert!(validate_expected_count(0).is_err());
        validate_inventory_case_count(MAX_INVENTORY_CASES).expect("case limit is inclusive");
        assert!(validate_inventory_case_count(MAX_INVENTORY_CASES + 1).is_err());
    }

    #[test]
    fn bounded_reader_rejects_oversized_artifact_without_modifying_it() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("artifact");
        fs::write(&path, "12345").expect("write fixture");

        let error = read_bounded_utf8(&path, 4, "fixture").expect_err("oversize must fail");

        assert!(
            error.to_string().contains("deterministic limit"),
            "{error:?}"
        );
        assert_eq!(fs::read_to_string(path).expect("read fixture"), "12345");
    }

    #[test]
    fn correlation_metadata_rejects_missing_or_malformed_attempt_times() {
        let mut report = crate::junit::JunitReport {
            run_id: Some("run".to_owned()),
            timestamp: Some("2026-08-13T12:00:00Z".to_owned()),
            cases: vec![case("seek", 0, false, 0.1)],
        };

        assert!(
            validate_correlation_metadata(&report)
                .expect_err("missing timestamp must fail")
                .contains("timestamp is missing")
        );
        report.cases[0].timestamp = Some("not-a-time".to_owned());
        assert!(
            validate_correlation_metadata(&report)
                .expect_err("malformed timestamp must fail")
                .contains("timestamp is malformed")
        );
        report.cases[0].timestamp = Some("2026-08-13T12:00:00Z".to_owned());
        report.timestamp = None;
        assert!(
            validate_correlation_metadata(&report)
                .expect_err("missing root timestamp must fail")
                .contains("root timestamp is missing")
        );
        report.timestamp = Some("not-a-time".to_owned());
        assert!(
            validate_correlation_metadata(&report)
                .expect_err("malformed root timestamp must fail")
                .contains("root timestamp is malformed")
        );
        report.timestamp = Some("2026-08-13T12:00:00Z".to_owned());
        report.run_id = None;
        assert!(
            validate_correlation_metadata(&report)
                .expect_err("missing run ID must fail")
                .contains("uuid is missing")
        );
    }

    #[test]
    fn missing_supplementary_evidence_marks_a_report_incomplete() {
        let mut markdown = "# Stress evidence\n\n- Result: **FAILED**\n".to_owned();

        mark_incomplete(&mut markdown);

        assert!(markdown.contains("Result: **INCOMPLETE**"), "{markdown}");
        assert!(!markdown.contains("Result: **FAILED**"), "{markdown}");
    }
}
