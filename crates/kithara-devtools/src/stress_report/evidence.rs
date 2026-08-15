//! Correlates per-attempt stress failures with runtime evidence.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    sync::LazyLock,
    time::Duration,
};

use regex::Regex;

use self::attempt::{AttemptKey, AttemptOutcome, attempt_outcomes};
use super::{MAX_FAILURE_ROWS, StressReportArgs, markdown_cell, test_id};
use crate::{common::project::StressEvidenceConfig, junit::CaseTiming};

mod attempt;
mod envelope;
mod line;
mod line_reader;
mod overlap;
mod pressure;

const MAX_SIGNATURE_ROWS: usize = 100;
const MAX_SIGNATURE_EXAMPLES: usize = 5;

#[derive(Debug, Default)]
struct SignatureCluster {
    failed_attempts: BTreeSet<String>,
    passed_attempts: BTreeSet<String>,
    unattributed_attempts: BTreeSet<String>,
    tests: BTreeSet<String>,
    details: BTreeSet<String>,
}

#[derive(Debug, Default)]
struct AttemptDossier {
    display: String,
    test: String,
    symptom: String,
    backtrace: String,
    wait_graph: BTreeSet<String>,
    lines: BTreeSet<String>,
    envelopes: BTreeSet<String>,
    pressure: String,
    co_runners: BTreeSet<String>,
}

impl SignatureCluster {
    fn attempts(&self) -> usize {
        self.failed_attempts
            .len()
            .saturating_add(self.passed_attempts.len())
            .saturating_add(self.unattributed_attempts.len())
    }
}

pub(super) fn append_correlated_evidence(
    out: &mut String,
    cases: &[CaseTiming],
    run_id: Option<&str>,
    args: &StressReportArgs,
) -> bool {
    let failed = cases.iter().filter(|case| case.failed).collect::<Vec<_>>();
    let expected_envelopes = failed
        .iter()
        .filter(|case| requires_envelope(&case.output, &args.evidence))
        .filter_map(|case| attempt_key(case))
        .collect::<BTreeSet<_>>();
    let outcomes = attempt_outcomes(cases);
    let dossier_keys = failed
        .iter()
        .filter_map(|case| attempt_key(case))
        .take(MAX_FAILURE_ROWS)
        .collect::<BTreeSet<_>>();
    let mut overlaps = overlap::for_targets(cases, &dossier_keys);
    let mut dossiers = failed
        .iter()
        .filter_map(|case| {
            let key = attempt_key(case)?;
            if !dossier_keys.contains(&key) {
                return None;
            }
            let dossier = AttemptDossier {
                display: attempt_id(case, run_id),
                test: test_id(case),
                symptom: failure_signature(case, &args.evidence),
                backtrace: backtrace_signature(&case.output, &args.evidence).unwrap_or_default(),
                wait_graph: wait_signatures(&case.output, &args.evidence)
                    .into_iter()
                    .collect(),
                co_runners: overlaps.remove(&key).unwrap_or_default(),
                ..AttemptDossier::default()
            };
            Some((key, dossier))
        })
        .collect::<BTreeMap<_, _>>();
    let mut symptoms = BTreeMap::new();
    let mut backtraces = BTreeMap::new();
    let mut waits = BTreeMap::new();
    let mut complete = true;
    for case in &failed {
        let attempt = attempt_id(case, run_id);
        let test = test_id(case);
        add_signature(
            &mut symptoms,
            failure_signature(case, &args.evidence),
            &attempt,
            &test,
            AttemptOutcome::Failed,
            None,
        );
        if let Some(signature) = backtrace_signature(&case.output, &args.evidence) {
            add_signature(
                &mut backtraces,
                signature,
                &attempt,
                &test,
                AttemptOutcome::Failed,
                None,
            );
        }
        for signature in wait_signatures(&case.output, &args.evidence) {
            add_signature(
                &mut waits,
                signature,
                &attempt,
                &test,
                AttemptOutcome::Failed,
                None,
            );
        }
    }

    render_clusters(
        out,
        "Failure symptom clusters",
        &symptoms,
        "The terminal panic or timeout. This locates the observed endpoint, not necessarily its cause.",
    );
    render_clusters(
        out,
        "Backtrace overlays",
        &backtraces,
        "The first project frames shared by failing attempts. Wrapper and address noise is removed.",
    );
    if let Some(path) = &args.line_log {
        complete &= line::append(
            out,
            path,
            args.evidence.line_marker.as_deref(),
            &outcomes,
            run_id,
            &mut dossiers,
        );
    }
    if let Some(path) = &args.envelope_dir {
        complete &= envelope::append(
            out,
            path,
            envelope::Input::new(&outcomes, &expected_envelopes, run_id, &args.evidence),
            &mut waits,
            &mut dossiers,
        );
    } else if !expected_envelopes.is_empty() {
        let _ = writeln!(
            out,
            "\nEvidence problem: `{}` timeout-class failed attempts require exact same-run attempt envelopes, but no envelope directory was provided.",
            expected_envelopes.len(),
        );
        complete = false;
    }
    render_clusters(
        out,
        "Wait-graph signatures",
        &waits,
        "Repeated holders, waiters, or quiescence pins are causal candidates. Task IDs and timing counters are removed. An optional backtrace belongs to the snapshot caller, not necessarily to a holder or waiter.",
    );
    if let Some(path) = &args.pressure_log {
        let (points, pressure_complete) = pressure::append(out, path);
        complete &= pressure_complete;
        pressure::correlate(&mut dossiers, cases, &points);
    }
    render_attempt_dossiers(out, &dossiers, failed.len());
    complete
}

fn attempt_key(case: &CaseTiming) -> Option<AttemptKey> {
    Some(AttemptKey {
        suite: case.suite.clone(),
        name: case.name.clone(),
        iteration: case.iteration?,
    })
}

fn add_signature(
    clusters: &mut BTreeMap<String, SignatureCluster>,
    signature: String,
    attempt: &str,
    test: &str,
    outcome: AttemptOutcome,
    detail: Option<&str>,
) {
    let cluster = clusters.entry(signature).or_default();
    match outcome {
        AttemptOutcome::Failed => &mut cluster.failed_attempts,
        AttemptOutcome::Passed => &mut cluster.passed_attempts,
        AttemptOutcome::Unattributed => &mut cluster.unattributed_attempts,
    }
    .insert(attempt.to_owned());
    if !test.is_empty() {
        cluster.tests.insert(test.to_owned());
    }
    if let Some(detail) = detail
        && cluster.details.len() < MAX_SIGNATURE_EXAMPLES
    {
        cluster.details.insert(markdown_cell(detail));
    }
}

fn render_clusters(
    out: &mut String,
    heading: &str,
    clusters: &BTreeMap<String, SignatureCluster>,
    explanation: &str,
) {
    if clusters.is_empty() {
        return;
    }
    let mut rows = clusters.iter().collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        Reverse(left.1.failed_attempts.len())
            .cmp(&Reverse(right.1.failed_attempts.len()))
            .then_with(|| Reverse(left.1.attempts()).cmp(&Reverse(right.1.attempts())))
            .then_with(|| left.0.cmp(right.0))
    });
    let _ = write!(
        out,
        "\n## {heading}\n\n{explanation}\n\n| signature | failed | passed | unattributed | tests | examples |\n|---|---:|---:|---:|---:|---|\n"
    );
    for (signature, cluster) in rows.iter().take(MAX_SIGNATURE_ROWS) {
        let examples = cluster
            .failed_attempts
            .iter()
            .chain(cluster.passed_attempts.iter())
            .chain(cluster.unattributed_attempts.iter())
            .take(MAX_SIGNATURE_EXAMPLES)
            .cloned()
            .collect::<Vec<_>>()
            .join("<br>");
        let detail = cluster
            .details
            .iter()
            .next()
            .map_or(String::new(), |detail| format!("<br>{detail}"));
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {} | {} | {}{} |",
            markdown_cell(signature),
            cluster.failed_attempts.len(),
            cluster.passed_attempts.len(),
            cluster.unattributed_attempts.len(),
            cluster.tests.len(),
            markdown_cell(&examples),
            detail,
        );
    }
    if rows.len() > MAX_SIGNATURE_ROWS {
        let _ = writeln!(
            out,
            "\nShowing the first {MAX_SIGNATURE_ROWS} of {} signatures. Raw artifacts are exhaustive.",
            rows.len()
        );
    }
}

fn attempt_id(case: &CaseTiming, run_id: Option<&str>) -> String {
    let iteration = case
        .iteration
        .map_or_else(|| "unknown".to_owned(), |value| value.to_string());
    run_id.map_or_else(
        || format!("{} {} @stress-{iteration}", case.suite, case.name),
        |run_id| format!("{run_id}:{}@stress-{iteration}${}", case.suite, case.name),
    )
}

fn failure_signature(case: &CaseTiming, evidence: &StressEvidenceConfig) -> String {
    let lines = clean_lines(&case.output);
    for (index, line) in lines.iter().enumerate() {
        if evidence
            .envelope_marker
            .as_deref()
            .is_some_and(|marker| line.contains(marker))
        {
            let summary = evidence
                .envelope_suffix_markers
                .iter()
                .filter_map(|marker| line.find(marker))
                .min()
                .map_or(line.as_str(), |index| &line[..index]);
            return normalize_signature(summary);
        }
        if is_timeout_line(line) {
            return normalize_signature(line);
        }
        if line.contains("panicked at") {
            let detail = lines
                .iter()
                .skip(index + 1)
                .find(|candidate| {
                    !candidate.is_empty()
                        && !candidate.starts_with("stack backtrace")
                        && !candidate.starts_with("note: run with")
                })
                .map_or(String::new(), |detail| format!(": {detail}"));
            return normalize_signature(&format!("{line}{detail}"));
        }
    }
    lines.first().map_or_else(
        || "failure output unavailable".to_owned(),
        |line| normalize_signature(line),
    )
}

fn requires_envelope(output: &str, evidence: &StressEvidenceConfig) -> bool {
    let lines = clean_lines(output);
    lines.first().is_some_and(|line| is_junit_timeout(line))
        || lines.iter().any(|line| {
            evidence
                .envelope_marker
                .as_deref()
                .is_some_and(|marker| line.contains(marker))
                || line.to_ascii_lowercase().contains("hard timeout")
        })
}

fn is_timeout_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    is_junit_timeout(&lower) || lower.contains("hard timeout") || lower.contains("timed out after")
}

fn is_junit_timeout(line: &str) -> bool {
    line == "test timeout" || line.starts_with("test timeout:")
}

pub(super) fn backtrace_signature(output: &str, evidence: &StressEvidenceConfig) -> Option<String> {
    static SOURCE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:[A-Za-z0-9_.-]+/)+[A-Za-z0-9_./-]+\.rs:\d+(?::\d+)?")
            .expect("source-location regex")
    });
    let clean = strip_ansi(output);
    let frames = SOURCE
        .find_iter(&clean)
        .map(|matched| matched.as_str().to_owned())
        .filter(|frame| {
            !evidence
                .source_excludes
                .iter()
                .any(|excluded| frame.contains(excluded))
        })
        .fold(Vec::<String>::new(), |mut frames, frame| {
            if frames.len() < MAX_SIGNATURE_EXAMPLES && !frames.contains(&frame) {
                frames.push(frame);
            }
            frames
        });
    (!frames.is_empty()).then(|| frames.join(" -> "))
}

fn wait_signatures(output: &str, evidence: &StressEvidenceConfig) -> Vec<String> {
    let mut signatures = BTreeSet::new();
    let mut context = "wait graph".to_owned();
    let mut primitive = None::<String>;
    let mut holder = None::<String>;
    for line in clean_lines(output) {
        let trimmed = line.trim();
        if let Some(value) = evidence
            .dump_marker
            .as_deref()
            .and_then(|marker| trimmed.split(marker).nth(1))
        {
            context = normalize_signature(value);
            continue;
        }
        if trimmed.starts_with('#')
            && evidence
                .primitive_marker
                .as_deref()
                .is_some_and(|marker| trimmed.contains(marker))
        {
            primitive = Some(normalize_wait(trimmed));
            holder = None;
            continue;
        }
        if evidence
            .holder_marker
            .as_deref()
            .is_some_and(|marker| trimmed.contains(marker))
        {
            holder = Some(normalize_wait(trimmed));
            continue;
        }
        if evidence
            .wait_marker
            .as_deref()
            .is_some_and(|marker| trimmed.contains(marker))
        {
            let edge = [
                Some(context.as_str()),
                primitive.as_deref(),
                holder.as_deref(),
                Some(trimmed),
            ]
            .into_iter()
            .flatten()
            .map(normalize_wait)
            .collect::<Vec<_>>()
            .join(" | ");
            signatures.insert(edge);
            continue;
        }
        if evidence
            .direct_markers
            .iter()
            .any(|needle| trimmed.contains(needle))
        {
            signatures.insert(format!("{} | {}", context, normalize_wait(trimmed)));
        }
    }
    signatures.into_iter().collect()
}

fn render_attempt_dossiers(
    out: &mut String,
    dossiers: &BTreeMap<AttemptKey, AttemptDossier>,
    failed_attempts: usize,
) {
    if dossiers.is_empty() {
        return;
    }
    out.push_str(
        "\n## Failed-attempt evidence overlay\n\nEach bounded example row joins the terminal symptom with same-attempt runtime evidence; raw artifacts remain exhaustive. Empty cells mean that source emitted no attributable record. Co-runners and pressure are correlation candidates, not causes.\n\n| attempt | symptom | project frames | wait graph | line evidence | envelope | pressure | co-running tests |\n|---|---|---|---|---|---|---|---|\n",
    );
    for dossier in dossiers.values().take(MAX_FAILURE_ROWS) {
        let _ = writeln!(
            out,
            "| `{}`<br>{} | {} | {} | {} | {} | {} | {} | {} |",
            markdown_cell(&dossier.display),
            markdown_cell(&dossier.test),
            markdown_cell(&dossier.symptom),
            markdown_cell(&dossier.backtrace),
            render_set(&dossier.wait_graph),
            render_set(&dossier.lines),
            render_set(&dossier.envelopes),
            markdown_cell(&dossier.pressure),
            render_set(&dossier.co_runners),
        );
    }
    if failed_attempts > dossiers.len() {
        let _ = writeln!(
            out,
            "\nShowing {} bounded examples of {failed_attempts} failed attempts. Raw artifacts are exhaustive.",
            dossiers.len(),
        );
    }
}

fn render_set(values: &BTreeSet<String>) -> String {
    let rendered = values
        .iter()
        .take(MAX_SIGNATURE_EXAMPLES)
        .map(|value| markdown_cell(value))
        .collect::<Vec<_>>()
        .join("<br>");
    if values.len() > MAX_SIGNATURE_EXAMPLES {
        format!("{rendered}<br>... ({} total)", values.len())
    } else {
        rendered
    }
}

fn clean_lines(text: &str) -> Vec<String> {
    strip_ansi(text)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

pub(super) fn strip_ansi(text: &str) -> String {
    static ANSI: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]").expect("ANSI escape regex"));
    ANSI.replace_all(text, "").into_owned()
}

fn normalize_signature(text: &str) -> String {
    static VOLATILE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b([A-Za-z_][A-Za-z0-9_]*(?:_ns|_ms)|pid|task|thread|id|dump)=[^\s,;]+")
            .expect("volatile diagnostic regex")
    });
    static HEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"0x[0-9a-fA-F]+").expect("address regex"));
    let text = strip_ansi(text).replace(['\r', '\n'], " ");
    let text = VOLATILE.replace_all(&text, "$1=<volatile>");
    let text = HEX.replace_all(&text, "0x<address>");
    markdown_cell(text.trim())
}

fn normalize_wait(text: &str) -> String {
    static IDS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?:#\d+|\b[A-Za-z_][A-Za-z0-9_]*id=[^\s]+|\b[A-Za-z_][A-Za-z0-9_:]*Key\([^)]*\))",
        )
        .expect("wait-graph identity regex")
    });
    let normalized = IDS.replace_all(text, "<id>");
    normalize_signature(&normalized)
}

fn duration_ms(secs: f64) -> u64 {
    let Ok(duration) = Duration::try_from_secs_f64(secs) else {
        return if secs.is_finite() && secs.is_sign_positive() {
            u64::MAX
        } else {
            1
        };
    };
    let millis = duration
        .as_millis()
        .saturating_add(u128::from(duration.subsec_nanos() % 1_000_000 != 0));
    u64::try_from(millis).unwrap_or(u64::MAX).max(1)
}

pub(super) fn parse_timestamp_ms(timestamp: &str) -> Option<u64> {
    static RFC3339: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d+))?(Z|[+-]\d{2}:\d{2})$",
        )
        .expect("RFC 3339 timestamp regex")
    });
    let captures = RFC3339.captures(timestamp)?;
    let read = |index| captures.get(index)?.as_str().parse::<i64>().ok();
    let year = read(1)?;
    let month = read(2)?;
    let day = read(3)?;
    let hour = read(4)?;
    let minute = read(5)?;
    let second = read(6)?;
    if !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    let millis = captures.get(7).map_or(0, |fraction| {
        let mut digits = fraction.as_str().chars();
        (0..3).fold(0_i64, |value, _| {
            value * 10
                + i64::from(
                    digits
                        .next()
                        .and_then(|digit| digit.to_digit(10))
                        .unwrap_or(0),
                )
        })
    });
    let zone = captures.get(8)?.as_str();
    let offset = if zone == "Z" {
        0
    } else {
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let hours = zone.get(1..3)?.parse::<i64>().ok()?;
        let minutes = zone.get(4..6)?.parse::<i64>().ok()?;
        if hours > 23 || minutes > 59 {
            return None;
        }
        sign * (hours * 3_600 + minutes * 60)
    };
    let seconds = days_from_civil(year, month, day)
        .checked_mul(86_400)?
        .checked_add(hour * 3_600 + minute * 60 + second)?
        .checked_sub(offset)?;
    let timestamp = seconds.checked_mul(1_000)?.checked_add(millis)?;
    u64::try_from(timestamp).ok()
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence() -> StressEvidenceConfig {
        StressEvidenceConfig {
            envelope_schema: Some("demo.hang.v1".to_owned()),
            envelope_marker: Some("[hang]".to_owned()),
            envelope_suffix_markers: vec![" payload=".to_owned(), " \u{2014} ".to_owned()],
            dump_marker: Some("[wait dump]".to_owned()),
            primitive_marker: Some("created_at=".to_owned()),
            holder_marker: Some("held by".to_owned()),
            wait_marker: Some("WAITING:".to_owned()),
            direct_markers: vec!["active holder".to_owned()],
            ..StressEvidenceConfig::default()
        }
    }

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

    #[test]
    fn timestamp_parser_handles_fractional_seconds_and_offsets() {
        assert_eq!(parse_timestamp_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_timestamp_ms("1970-01-01T01:00:00.123456+01:00"),
            Some(123)
        );
        assert_eq!(parse_timestamp_ms("2000-02-30T00:00:00Z"), None);
        assert_eq!(parse_timestamp_ms("not-a-timestamp"), None);
    }

    #[test]
    fn duration_rounds_up_to_a_positive_millisecond() {
        assert_eq!(duration_ms(0.0), 1);
        assert_eq!(duration_ms(0.000_001), 1);
        assert_eq!(duration_ms(0.001), 1);
        assert_eq!(duration_ms(0.001_001), 2);
        assert_eq!(duration_ms(1.0), 1_000);
        assert_eq!(duration_ms(f64::NAN), 1);
        assert_eq!(duration_ms(f64::MAX), u64::MAX);
    }

    #[test]
    fn signature_normalization_keeps_field_names_and_removes_jitter() {
        let normalized = normalize_signature("pid=42 task=7 address=0xfeed");
        assert_eq!(
            normalized,
            "pid=<volatile> task=<volatile> address=0x<address>"
        );
    }

    #[test]
    fn hang_symptom_ignores_embedded_envelope_and_dump_filename() {
        let mut failed = case("seek", 0, true, 1.0);
        failed.output = "[hang] detected: pre-kill ts_ms=42 pid=7 dump=/tmp/hang-42.json [still running] \u{2014} {\"nextest\":{\"attempt_id\":\"run:a\"}}".to_owned();

        let signature = failure_signature(&failed, &evidence());

        assert_eq!(
            signature,
            "[hang] detected: pre-kill ts_ms=<volatile> pid=<volatile> dump=<volatile> [still running]"
        );
        assert!(!signature.contains("attempt_id"));
    }

    #[test]
    fn hang_symptom_ignores_ascii_embedded_envelope() {
        let mut failed = case("seek", 0, true, 1.0);
        failed.output =
            "[hang] detected: pre-kill ts_ms=42 payload={\"nextest\":{\"attempt_id\":\"run:a\"}}"
                .to_owned();

        assert_eq!(
            failure_signature(&failed, &evidence()),
            "[hang] detected: pre-kill ts_ms=<volatile>"
        );
    }

    #[test]
    fn nextest_timeout_class_requires_an_envelope_without_marker() {
        let evidence = evidence();
        assert!(requires_envelope(
            "test timeout: after 120s: process did not exit",
            &evidence,
        ));
        assert!(requires_envelope(
            "assertion failed\nHARD TIMEOUT after 30s",
            &evidence,
        ));
        assert!(!requires_envelope(
            "test failure: timeout setting wrong",
            &evidence,
        ));
        assert!(!requires_envelope(
            "test failure: assertion timed out after 120ms",
            &evidence,
        ));
    }
}
