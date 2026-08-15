//! Reading a `JUnit` report for performance and stress evidence consumers.

use anyhow::{Context, Result, bail};

const MAX_JUNIT_CASES: usize = 750_000;
const MAX_CASE_OUTPUT_BYTES: usize = 8 * 1_024 * 1_024;

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct CaseTiming {
    pub name: String,
    pub suite: String,
    pub iteration: Option<usize>,
    pub failed: bool,
    pub secs: f64,
    pub timestamp: Option<String>,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub(crate) struct JunitReport {
    pub(crate) run_id: Option<String>,
    pub(crate) timestamp: Option<String>,
    pub(crate) cases: Vec<CaseTiming>,
}

/// # Errors
///
/// Fails when the document is not `XML`, a `testcase` identity is empty, a
/// stress suite does not own its testcase, or a `time` attribute is missing,
/// negative, non-finite, or not a number. A skipped stress testcase is also
/// rejected because it is not per-iteration execution evidence. Testcase and
/// retained-output limits reject evidence that cannot be analyzed safely.
pub fn parse_junit(xml: &str) -> Result<Vec<CaseTiming>> {
    parse_junit_report(xml).map(|report| report.cases)
}

/// Reads run-level identity and testcase evidence from nextest `JUnit`.
///
/// # Errors
///
/// Fails under the same conditions as [`parse_junit`].
pub(crate) fn parse_junit_report(xml: &str) -> Result<JunitReport> {
    let doc = roxmltree::Document::parse(xml).context("parse junit xml")?;
    let root = doc
        .descendants()
        .find(|node| node.has_tag_name("testsuites"));
    let run_id = root
        .and_then(|node| node.attribute("uuid"))
        .map(str::to_owned);
    let timestamp = root
        .and_then(|node| node.attribute("timestamp"))
        .map(str::to_owned);
    let mut cases = Vec::new();
    for node in doc.descendants().filter(|n| n.has_tag_name("testcase")) {
        validate_case_count(cases.len().saturating_add(1))?;
        let name = node.attribute("name").unwrap_or_default().to_owned();
        let suite = node.attribute("classname").unwrap_or_default().to_owned();
        if name.trim().is_empty() {
            bail!("testcase name is empty");
        }
        if suite.trim().is_empty() {
            bail!("testcase classname is empty");
        }
        let parent_suite = node
            .ancestors()
            .find(|ancestor| ancestor.has_tag_name("testsuite"))
            .and_then(|parent| parent.attribute("name"));
        let iteration = parent_suite
            .and_then(stress_suite)
            .map(|(base, iteration)| {
                if base != suite.as_str() {
                    bail!("stress testsuite base does not match testcase classname");
                }
                Ok(iteration)
            })
            .transpose()?;
        let secs: f64 = node
            .attribute("time")
            .context("testcase time attribute is missing")?
            .parse()
            .with_context(|| format!("bad time attribute on {suite} {name}"))?;
        if !secs.is_finite() || secs < 0.0 {
            bail!("invalid time attribute on {suite} {name}");
        }
        let stress = iteration.is_some();
        if stress && node.children().any(|child| child.has_tag_name("skipped")) {
            bail!("selected testcase {suite} {name} was skipped");
        }
        let failed = node
            .children()
            .any(|c| c.has_tag_name("failure") || c.has_tag_name("error"));
        let timestamp = node.attribute("timestamp").map(str::to_owned);
        let output = failure_output(node)
            .with_context(|| format!("retain failure output on {suite} {name}"))?;
        cases.push(CaseTiming {
            name,
            suite,
            iteration,
            failed,
            secs,
            timestamp,
            output,
        });
    }
    Ok(JunitReport {
        run_id,
        timestamp,
        cases,
    })
}

fn validate_case_count(count: usize) -> Result<()> {
    if count > MAX_JUNIT_CASES {
        bail!("JUnit exceeds the deterministic limit of {MAX_JUNIT_CASES} testcases");
    }
    Ok(())
}

fn failure_output(node: roxmltree::Node<'_, '_>) -> Result<String> {
    let mut output = String::new();
    for child in node
        .children()
        .filter(|child| child.has_tag_name("failure") || child.has_tag_name("error"))
    {
        append_failure_description(&mut output, child)?;
    }
    for text in node
        .children()
        .filter(|child| child.has_tag_name("system-out") || child.has_tag_name("system-err"))
        .filter_map(|child| child.text())
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        append_output(&mut output, "\n", text)?;
    }
    Ok(output)
}

fn append_failure_description(output: &mut String, node: roxmltree::Node<'_, '_>) -> Result<()> {
    let kind = node
        .attribute("type")
        .unwrap_or_else(|| node.tag_name().name());
    let message = node.attribute("message").unwrap_or_default().trim();
    let body = node.text().unwrap_or_default().trim();
    // A Rust panic puts the same header in both: nextest lifts the first line
    // of the body into `message`. Keeping both spent the retained output — and
    // every signature derived from it — on saying the header twice, which
    // pushed the assertion's own values past the width a report row has.
    let message = if body.starts_with(message) {
        ""
    } else {
        message
    };
    let mut first = true;
    for part in [kind, message, body]
        .into_iter()
        .filter(|part| !part.is_empty())
    {
        let separator = if first {
            first = false;
            "\n"
        } else {
            ": "
        };
        append_output(output, separator, part)?;
    }
    Ok(())
}

fn append_output(output: &mut String, separator: &str, text: &str) -> Result<()> {
    let separator = if output.is_empty() { "" } else { separator };
    let new_len = output
        .len()
        .checked_add(separator.len())
        .and_then(|length| length.checked_add(text.len()))
        .context("testcase retained output length overflow")?;
    if new_len > MAX_CASE_OUTPUT_BYTES {
        bail!(
            "testcase retained failure output exceeds the deterministic limit of {MAX_CASE_OUTPUT_BYTES} bytes"
        );
    }
    output.push_str(separator);
    output.push_str(text);
    Ok(())
}

fn stress_suite(suite: &str) -> Option<(&str, usize)> {
    let (base, iteration) = suite.rsplit_once("@stress-")?;
    let iteration = iteration.parse().ok()?;
    Some((base, iteration))
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="2" failures="1">
  <testsuite name="demo-tests::suite_light" tests="2" failures="1">
    <testcase name="offline::gapless" classname="demo-tests::suite_light" time="1.532"/>
    <testcase name="offline::seek" classname="demo-tests::suite_light" time="0.201">
      <failure type="test failure">boom</failure>
    </testcase>
  </testsuite>
</testsuites>"#;

    /// What nextest writes for a test that failed an attempt and passed a later one.
    const RETRIED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="1" failures="0">
  <testsuite name="demo-tests::suite_stress" tests="1" failures="0">
    <testcase name="abr::switch" classname="demo-tests::suite_stress" time="2.100">
      <flakyFailure type="test failure" message="boom"/>
    </testcase>
  </testsuite>
</testsuites>"#;

    const STRESS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="1" failures="1">
  <testsuite name="demo-tests::suite_stress@stress-7" tests="1" failures="1">
    <testcase name="offline::seek" classname="demo-tests::suite_stress" time="0.201" timestamp="2026-08-13T12:34:56.789Z">
      <failure type="test failure">boom</failure>
    </testcase>
  </testsuite>
</testsuites>"#;

    /// What nextest writes for a failed `assert_eq!`: the panic header is
    /// lifted into `message` and the body repeats it verbatim.
    const PANIC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="1" failures="1">
  <testsuite name="demo-tests::suite_light" tests="1" failures="1">
    <testcase name="audio::warms_pool" classname="demo-tests::suite_light" time="0.536">
      <failure message="thread 'audio::warms_pool' (971370) panicked at tests/demo.rs:166:5" type="test failure with exit code 101">thread 'audio::warms_pool' (971370) panicked at tests/demo.rs:166:5:
assertion `left == right` failed: a warmed pool must serve decode-sized buffers without allocating
  left: 0
 right: 1
stack backtrace:
   0: __rustc::rust_begin_unwind</failure>
    </testcase>
  </testsuite>
</testsuites>"#;

    /// The header is worth keeping once. Kept twice it consumed the retained
    /// output a report row can show, and what fell off the end was the only
    /// part that says which defect this is: the assertion's own values.
    #[test]
    fn a_panic_header_lifted_into_the_message_is_not_kept_twice() {
        let cases = parse_junit(PANIC).expect("parse junit");

        let output = &cases[0].output;
        assert_eq!(output.matches("panicked at").count(), 1, "{output}");
        assert!(output.contains("left: 0"), "{output}");
        assert!(output.contains("right: 1"), "{output}");
    }

    /// A message the body does NOT repeat still carries information, and
    /// dropping it would lose the only description such a failure has.
    #[test]
    fn a_message_the_body_does_not_repeat_is_kept() {
        let cases = parse_junit(&PANIC.replace(
            "message=\"thread 'audio::warms_pool' (971370) panicked at tests/demo.rs:166:5\"",
            "message=\"the runner killed it\"",
        ))
        .expect("parse junit");

        assert!(
            cases[0].output.contains("the runner killed it"),
            "{}",
            cases[0].output
        );
    }

    /// Retries buy nothing if a passed-on-retry case still reads as failed.
    #[test]
    fn a_test_that_passed_on_a_retry_is_not_a_failure() {
        let cases = parse_junit(RETRIED).expect("parse junit");

        assert_eq!(cases.len(), 1);
        assert!(!cases[0].failed);
    }

    #[test]
    fn parses_cases_and_failures() {
        let cases = parse_junit(XML).expect("parse junit");

        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].suite, "demo-tests::suite_light");
        assert_eq!(cases[0].name, "offline::gapless");
        assert_eq!(cases[0].iteration, None);
        assert!((cases[0].secs - 1.532).abs() < 1e-9);
        assert!(!cases[0].failed);
        assert!(cases[1].failed);
        assert_eq!(cases[1].output, "test failure: boom");
    }

    #[test]
    fn retains_the_zero_based_stress_iteration() {
        let report = parse_junit_report(STRESS).expect("parse stress junit");
        let cases = report.cases;

        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].iteration, Some(7));
        assert!(cases[0].failed);
        assert_eq!(
            cases[0].timestamp.as_deref(),
            Some("2026-08-13T12:34:56.789Z")
        );
    }

    #[test]
    fn retains_run_identity_when_nextest_provides_it() {
        let xml = r#"<testsuites uuid="run-id" timestamp="2026-08-13T12:34:56+00:00">
  <testsuite name="demo@stress-0">
    <testcase name="seek" classname="demo" time="0.1"/>
  </testsuite>
</testsuites>"#;

        let report = parse_junit_report(xml).expect("parse report identity");

        assert_eq!(report.run_id.as_deref(), Some("run-id"));
        assert_eq!(
            report.timestamp.as_deref(),
            Some("2026-08-13T12:34:56+00:00")
        );
    }

    #[test]
    fn rejects_empty_testcase_identity() {
        for xml in [
            r#"<testsuite name="demo@stress-0"><testcase classname="demo"/></testsuite>"#,
            r#"<testsuite name="demo@stress-0"><testcase name="seek"/></testsuite>"#,
        ] {
            let error = parse_junit(xml).expect_err("empty identity must be rejected");

            assert!(error.to_string().contains("is empty"), "{error:?}");
        }
    }

    #[test]
    fn rejects_mismatched_stress_suite_identity() {
        let xml = r#"<testsuite name="other@stress-0">
  <testcase name="seek" classname="demo"/>
</testsuite>"#;

        let error = parse_junit(xml).expect_err("suite mismatch must be rejected");

        assert!(
            error
                .to_string()
                .contains("testsuite base does not match testcase classname"),
            "{error:?}"
        );
    }

    #[test]
    fn rejects_missing_or_invalid_timing() {
        for time in [None, Some("-1"), Some("NaN"), Some("inf"), Some("bad")] {
            let attribute = time.map_or_else(String::new, |time| format!(r#" time="{time}""#));
            let xml = format!(
                r#"<testsuite name="demo@stress-0"><testcase name="seek" classname="demo"{attribute}/></testsuite>"#
            );

            let error = parse_junit(&xml).expect_err("invalid timing must be rejected");

            assert!(error.to_string().contains("time attribute"), "{error:?}");
        }
    }

    #[test]
    fn rejects_a_selected_testcase_that_was_skipped() {
        let xml = r#"<testsuite name="demo@stress-0">
  <testcase name="seek" classname="demo" time="0.1"><skipped/></testcase>
</testsuite>"#;

        let error = parse_junit(xml).expect_err("skipped evidence is incomplete");

        assert!(error.to_string().contains("was skipped"), "{error:?}");
    }

    #[test]
    fn keeps_failure_kind_message_and_captured_streams() {
        let xml = r#"<testsuite name="demo@stress-0">
  <testcase name="seek" classname="demo" time="0.1">
    <failure type="test timeout" message="after 120s">watchdog</failure>
    <system-err>stack backtrace</system-err>
  </testcase>
</testsuite>"#;

        let cases = parse_junit(xml).expect("parse failed case");

        assert_eq!(
            cases[0].output,
            "test timeout: after 120s: watchdog\nstack backtrace"
        );
    }

    #[test]
    fn testcase_and_retained_output_limits_are_inclusive() {
        validate_case_count(MAX_JUNIT_CASES).expect("case limit is inclusive");
        assert!(validate_case_count(MAX_JUNIT_CASES + 1).is_err());

        let mut output = "x".repeat(MAX_CASE_OUTPUT_BYTES);
        let error = append_output(&mut output, "\n", "y").expect_err("output must be bounded");

        assert!(
            error.to_string().contains("deterministic limit"),
            "{error:?}"
        );
        assert_eq!(output.len(), MAX_CASE_OUTPUT_BYTES);
    }
}
