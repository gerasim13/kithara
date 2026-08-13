//! Reading a `JUnit` report. Both the perf matrix and the CI verdict need
//! one, so it belongs to the crate rather than to either of them.

use anyhow::{Context, Result};

#[derive(Clone, Debug, PartialEq)]
pub struct CaseTiming {
    pub name: String,
    pub suite: String,
    pub failed: bool,
    pub secs: f64,
}

/// # Errors
///
/// Fails when the document is not XML, or when a `testcase` carries a
/// `time` attribute that is not a number.
pub fn parse_junit(xml: &str) -> Result<Vec<CaseTiming>> {
    let doc = roxmltree::Document::parse(xml).context("parse junit xml")?;
    let mut cases = Vec::new();
    for node in doc.descendants().filter(|n| n.has_tag_name("testcase")) {
        let name = node.attribute("name").unwrap_or_default().to_owned();
        let suite = node.attribute("classname").unwrap_or_default().to_owned();
        let secs: f64 = node
            .attribute("time")
            .unwrap_or("0")
            .parse()
            .with_context(|| format!("bad time attribute on {suite} {name}"))?;
        let failed = node
            .children()
            .any(|c| c.has_tag_name("failure") || c.has_tag_name("error"));
        cases.push(CaseTiming {
            name,
            suite,
            failed,
            secs,
        });
    }
    Ok(cases)
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
        assert!((cases[0].secs - 1.532).abs() < 1e-9);
        assert!(!cases[0].failed);
        assert!(cases[1].failed);
    }
}
