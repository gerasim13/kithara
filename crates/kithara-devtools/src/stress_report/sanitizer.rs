//! Reads a sanitizer's own findings out of a command lane's log.
//!
//! A sanitizer aborts the process at the offending call, so the lane has no
//! per-test results — but the report it prints names the violated contract, the
//! function that violated it, and the stack that got there. Counting only exit
//! codes throws all of that away and leaves the reader to open the log and
//! guess. Attributing each finding to the attempt it came from turns a
//! violation that fires intermittently into a rate, which is the same thing the
//! per-test lanes report and the reason the lane is in a campaign at all.

use std::collections::{BTreeMap, BTreeSet};

use super::evidence::{backtrace_signature, strip_ansi};
use crate::common::project::StressEvidenceConfig;

/// Written to the lane log before each attempt so findings can be attributed.
pub(crate) const ATTEMPT_MARKER: &str = "[kithara_stress] attempt ";

/// Every distinct finding, and the attempts it appeared in.
pub(crate) type Findings = BTreeMap<String, BTreeSet<usize>>;

/// Groups a log's sanitizer findings by what they say, not by how often the
/// process died.
pub(crate) fn findings(log: &str, evidence: &StressEvidenceConfig) -> Findings {
    let clean = strip_ansi(log);
    let mut findings = Findings::new();
    let mut attempt = 0;
    let mut block: Option<Block> = None;
    for line in clean.lines() {
        if let Some(index) = attempt_index(line) {
            finish(&mut block, attempt, &mut findings, evidence);
            attempt = index;
            continue;
        }
        if let Some(kind) = error_kind(line) {
            finish(&mut block, attempt, &mut findings, evidence);
            block = Some(Block::new(kind));
            continue;
        }
        let Some(open) = block.as_mut() else {
            continue;
        };
        if open.push(line) {
            continue;
        }
        finish(&mut block, attempt, &mut findings, evidence);
    }
    finish(&mut block, attempt, &mut findings, evidence);
    findings
}

struct Block {
    kind: String,
    call: Option<String>,
    body: String,
}

impl Block {
    fn new(kind: String) -> Self {
        Self {
            kind,
            call: None,
            body: String::new(),
        }
    }

    /// Takes one line of the sanitizer's report; false once the report ends.
    ///
    /// The report runs from its `ERROR:` header to the first blank line after
    /// it. Test-runner output resumes there, and reading on would attribute an
    /// unrelated source location to the violation.
    fn push(&mut self, line: &str) -> bool {
        if line.trim().is_empty() {
            return false;
        }
        if self.call.is_none() {
            self.call = quoted(line);
        }
        self.body.push_str(line);
        self.body.push('\n');
        true
    }

    fn signature(&self, evidence: &StressEvidenceConfig) -> String {
        let call = self.call.as_deref().unwrap_or("unnamed call");
        backtrace_signature(&self.body, evidence).map_or_else(
            || format!("{} `{call}`", self.kind),
            |frames| format!("{} `{call}` at {frames}", self.kind),
        )
    }
}

fn finish(
    block: &mut Option<Block>,
    attempt: usize,
    findings: &mut Findings,
    evidence: &StressEvidenceConfig,
) {
    if let Some(open) = block.take() {
        findings
            .entry(open.signature(evidence))
            .or_default()
            .insert(attempt);
    }
}

/// The contract a sanitizer says was violated, from its report header.
///
/// The header names the sanitizer and then the violation, and both matter: a
/// campaign may repeat more than one sanitizer, and a finding that did not say
/// which one it came from could not be read.
fn error_kind(line: &str) -> Option<String> {
    let (_, tail) = line.split_once("ERROR: ")?;
    let (sanitizer, violation) = tail.split_once(": ")?;
    let violation = violation.trim();
    (!sanitizer.trim().is_empty() && !violation.is_empty())
        .then(|| format!("{}: {violation}", sanitizer.trim()))
}

fn attempt_index(line: &str) -> Option<usize> {
    line.split_once(ATTEMPT_MARKER)?.1.trim().parse().ok()
}

fn quoted(line: &str) -> Option<String> {
    let (_, tail) = line.split_once('`')?;
    let (inside, _) = tail.split_once('`')?;
    (!inside.trim().is_empty()).then(|| inside.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNSAFE_CALL: &str = "\
==2534==ERROR: RealtimeSanitizer: unsafe-library-call
Intercepted call to real-time unsafe function `malloc` in real-time context!
    #0 0x5628d3a1b2c0 in malloc (/opt/bin/suite_light+0x1042c0)
    #1 0x5628d3b0e1a4 in alloc::alloc::alloc /rustc/abc/library/alloc/src/alloc.rs:100:9
    #2 0x5628d3c11f30 in kithara_audio::renderer::mix crates/kithara-audio/src/renderer/mix.rs:214:23

test kithara_play::rt_metrics::a_healthy_track_reports_no_trouble ... ok
";

    const BLOCKING_CALL: &str = "\
==2534==ERROR: RealtimeSanitizer: blocking-call
Call to blocking function `pthread_mutex_lock` in real-time context!
    #0 0x5628d3a1b2c0 in pthread_mutex_lock (/opt/bin/suite_light+0x1042c0)
    #1 0x5628d3c11f30 in kithara_audio::renderer::slot crates/kithara-audio/src/renderer/slot.rs:88:9
";

    fn evidence() -> StressEvidenceConfig {
        StressEvidenceConfig::default()
    }

    #[test]
    fn a_finding_names_the_violated_contract_the_call_and_where_it_came_from() {
        let found = findings(UNSAFE_CALL, &evidence());

        let (signature, attempts) = found.iter().next().expect("one finding");
        assert!(signature.contains("RealtimeSanitizer: unsafe-library-call"));
        assert!(signature.contains("`malloc`"));
        assert!(signature.contains("crates/kithara-audio/src/renderer/mix.rs:214:23"));
        assert_eq!(attempts, &BTreeSet::from([0]));
    }

    /// A violation that fires on some attempts and not others is the whole
    /// reason the lane is repeated. Reporting it without saying which attempts
    /// would leave a one-in-fifty violation looking the same as a certain one.
    #[test]
    fn the_same_violation_on_two_attempts_is_one_finding_with_both_attempts() {
        let log = format!("{ATTEMPT_MARKER}0\n{UNSAFE_CALL}\n{ATTEMPT_MARKER}3\n{UNSAFE_CALL}");

        let found = findings(&log, &evidence());

        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            found.values().next().expect("one finding"),
            &BTreeSet::from([0, 3])
        );
    }

    #[test]
    fn two_different_violations_stay_apart() {
        let log = format!("{UNSAFE_CALL}\n{BLOCKING_CALL}");

        let found = findings(&log, &evidence());

        assert_eq!(found.len(), 2, "{found:?}");
    }

    /// The report ends at the blank line after it. Reading past that would
    /// attribute the next test's source locations to this violation.
    #[test]
    fn output_after_the_report_does_not_join_the_finding() {
        let found = findings(UNSAFE_CALL, &evidence());

        let signature = found.keys().next().expect("one finding");
        assert!(!signature.contains("rt_metrics"), "{signature}");
    }

    #[test]
    fn a_log_the_sanitizer_never_wrote_to_yields_nothing() {
        assert!(findings("running 14 tests\ntest result: ok.\n", &evidence()).is_empty());
    }
}
