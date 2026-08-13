//! Correlates per-attempt stress failures with runtime evidence.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    sync::LazyLock,
};

use regex::Regex;

use self::attempt::{AttemptKey, AttemptOutcome, attempt_outcomes};
use super::{MAX_FAILURE_ROWS, StressReportArgs, markdown_cell, test_id};
use crate::junit::CaseTiming;

mod attempt;
mod hang;
mod line_reader;
mod no_block;
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
    flash: BTreeSet<String>,
    no_block: BTreeSet<String>,
    hang: BTreeSet<String>,
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
    let expected_hang = failed
        .iter()
        .filter(|case| requires_hang_envelope(&case.output))
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
                symptom: failure_signature(case),
                backtrace: backtrace_signature(&case.output).unwrap_or_default(),
                flash: flash_signatures(&case.output).into_iter().collect(),
                co_runners: overlaps.remove(&key).unwrap_or_default(),
                ..AttemptDossier::default()
            };
            Some((key, dossier))
        })
        .collect::<BTreeMap<_, _>>();
    let mut symptoms = BTreeMap::new();
    let mut backtraces = BTreeMap::new();
    let mut flash = BTreeMap::new();
    let mut complete = true;
    for case in &failed {
        let attempt = attempt_id(case, run_id);
        let test = test_id(case);
        add_signature(
            &mut symptoms,
            failure_signature(case),
            &attempt,
            &test,
            AttemptOutcome::Failed,
            None,
        );
        if let Some(signature) = backtrace_signature(&case.output) {
            add_signature(
                &mut backtraces,
                signature,
                &attempt,
                &test,
                AttemptOutcome::Failed,
                None,
            );
        }
        for signature in flash_signatures(&case.output) {
            add_signature(
                &mut flash,
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
    if let Some(path) = &args.no_block_log {
        complete &= no_block::append(out, path, &outcomes, run_id, &mut dossiers);
    }
    if let Some(path) = &args.hang_dir {
        complete &= hang::append(
            out,
            path,
            &outcomes,
            &expected_hang,
            run_id,
            &mut flash,
            &mut dossiers,
        );
    } else if !expected_hang.is_empty() {
        let _ = writeln!(
            out,
            "\nEvidence problem: `{}` timeout-class failed attempts require exact same-run hang envelopes, but no hang directory was provided.",
            expected_hang.len(),
        );
        complete = false;
    }
    render_clusters(
        out,
        "Flash wait signatures",
        &flash,
        "Repeated holders, waiters, or quiescence pins are causal candidates. Task IDs and timing counters are removed. An optional backtrace belongs to the dump caller, not necessarily to a holder or waiter.",
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

fn failure_signature(case: &CaseTiming) -> String {
    let lines = clean_lines(&case.output);
    for (index, line) in lines.iter().enumerate() {
        if line.contains("[kithara_hang_detector]") {
            let summary = line
                .split(" payload=")
                .next()
                .unwrap_or(line.as_str())
                .split(" \u{2014} ")
                .next()
                .unwrap_or(line.as_str());
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

fn requires_hang_envelope(output: &str) -> bool {
    let lines = clean_lines(output);
    lines.first().is_some_and(|line| is_junit_timeout(line))
        || lines.iter().any(|line| {
            line.contains("[kithara_hang_detector]")
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

fn backtrace_signature(output: &str) -> Option<String> {
    static SOURCE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:crates|tests|xtask)/[A-Za-z0-9_./-]+\.rs:\d+(?::\d+)?")
            .expect("source-location regex")
    });
    let clean = strip_ansi(output);
    let frames = SOURCE
        .find_iter(&clean)
        .map(|matched| matched.as_str().to_owned())
        .filter(|frame| {
            !frame.contains("kithara-test-macros/")
                && !frame.contains("kithara-test-utils/src/test/")
        })
        .fold(Vec::<String>::new(), |mut frames, frame| {
            if frames.len() < MAX_SIGNATURE_EXAMPLES && !frames.contains(&frame) {
                frames.push(frame);
            }
            frames
        });
    (!frames.is_empty()).then(|| frames.join(" -> "))
}

fn flash_signatures(output: &str) -> Vec<String> {
    let mut signatures = BTreeSet::new();
    let mut context = "flash hang".to_owned();
    let mut primitive = None::<String>;
    let mut holder = None::<String>;
    for line in clean_lines(output) {
        let trimmed = line.trim();
        if let Some(value) = trimmed.split("[flash hang dump]").nth(1) {
            context = normalize_signature(value);
            continue;
        }
        if trimmed.starts_with('#') && trimmed.contains("created_at=") {
            primitive = Some(normalize_flash(trimmed));
            holder = None;
            continue;
        }
        if trimmed.contains("held by") {
            holder = Some(normalize_flash(trimmed));
            continue;
        }
        if trimmed.contains("WAITING:") {
            let edge = [
                Some(context.as_str()),
                primitive.as_deref(),
                holder.as_deref(),
                Some(trimmed),
            ]
            .into_iter()
            .flatten()
            .map(normalize_flash)
            .collect::<Vec<_>>()
            .join(" | ");
            signatures.insert(edge);
            continue;
        }
        if [
            "active_async holder",
            "active holder thread=",
            "engine core lock held",
            "BRIDGED sync wait",
            "state lock held",
        ]
        .iter()
        .any(|needle| trimmed.contains(needle))
        {
            signatures.insert(format!("{} | {}", context, normalize_flash(trimmed)));
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
        "\n## Failed-attempt evidence overlay\n\nEach bounded example row joins the terminal symptom with same-attempt runtime evidence; raw artifacts remain exhaustive. Empty cells mean that source emitted no attributable record. Co-runners and pressure are correlation candidates, not causes.\n\n| attempt | symptom | project frames | Flash | no-block | hang | pressure | co-running tests |\n|---|---|---|---|---|---|---|---|\n",
    );
    for dossier in dossiers.values().take(MAX_FAILURE_ROWS) {
        let _ = writeln!(
            out,
            "| `{}`<br>{} | {} | {} | {} | {} | {} | {} | {} |",
            markdown_cell(&dossier.display),
            markdown_cell(&dossier.test),
            markdown_cell(&dossier.symptom),
            markdown_cell(&dossier.backtrace),
            render_set(&dossier.flash),
            render_set(&dossier.no_block),
            render_set(&dossier.hang),
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

fn strip_ansi(text: &str) -> String {
    static ANSI: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]").expect("ANSI escape regex"));
    ANSI.replace_all(text, "").into_owned()
}

fn normalize_signature(text: &str) -> String {
    static VOLATILE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"\b(ts_ms|pid|task|thread|id|dump|running_for_ns|deadline_in_ns|virtual_now_ns)=[^\s,;]+",
        )
        .expect("volatile diagnostic regex")
    });
    static HEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"0x[0-9a-fA-F]+").expect("address regex"));
    let text = strip_ansi(text).replace(['\r', '\n'], " ");
    let text = VOLATILE.replace_all(&text, "$1=<volatile>");
    let text = HEX.replace_all(&text, "0x<address>");
    markdown_cell(text.trim())
}

fn normalize_flash(text: &str) -> String {
    static IDS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:#\d+|\bcvid=\d+|\btask=\d+|\bid=[^\s]+|ThreadKey\([^)]*\))")
            .expect("Flash identity regex")
    });
    let normalized = IDS.replace_all(text, "<id>");
    normalize_signature(&normalized)
}

fn duration_ms(secs: f64) -> u64 {
    (secs * 1_000.0).ceil().max(1.0) as u64
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
                + digits
                    .next()
                    .and_then(|digit| digit.to_digit(10))
                    .unwrap_or(0) as i64
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
        failed.output = "[kithara_hang_detector] hang detected: pre-kill ts_ms=42 pid=7 dump=/tmp/kithara-hang-42.json [still running] \u{2014} {\"nextest\":{\"attempt_id\":\"run:a\"}}".to_owned();

        let signature = failure_signature(&failed);

        assert_eq!(
            signature,
            "[kithara_hang_detector] hang detected: pre-kill ts_ms=<volatile> pid=<volatile> dump=<volatile> [still running]"
        );
        assert!(!signature.contains("attempt_id"));
    }

    #[test]
    fn hang_symptom_ignores_ascii_embedded_envelope() {
        let mut failed = case("seek", 0, true, 1.0);
        failed.output = "[kithara_hang_detector] hang detected: pre-kill ts_ms=42 payload={\"nextest\":{\"attempt_id\":\"run:a\"}}".to_owned();

        assert_eq!(
            failure_signature(&failed),
            "[kithara_hang_detector] hang detected: pre-kill ts_ms=<volatile>"
        );
    }

    #[test]
    fn nextest_timeout_class_requires_a_hang_envelope_without_marker() {
        assert!(requires_hang_envelope(
            "test timeout: after 120s: process did not exit"
        ));
        assert!(requires_hang_envelope(
            "assertion failed\nHARD TIMEOUT after 30s"
        ));
        assert!(!requires_hang_envelope(
            "test failure: timeout setting wrong"
        ));
        assert!(!requires_hang_envelope(
            "test failure: assertion timed out after 120ms"
        ));
    }
}
