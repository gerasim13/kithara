use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use kithara_devtools::junit::{CaseTiming, parse_junit};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// How many `main` runs the journal keeps. One is not enough: a test that fails
/// a quarter of the time would otherwise land in a branch's column whenever the
/// single remembered run happened to be green, and block on its own noise.
const REMEMBERED_RUNS: usize = 5;

#[derive(Debug, Args)]
pub(crate) struct VerdictArgs {
    #[command(subcommand)]
    command: VerdictCommand,
}

#[derive(Debug, Subcommand)]
enum VerdictCommand {
    /// Record what the default branch failed, for later runs to compare against.
    Record {
        #[command(flatten)]
        common: Common,
        /// Commit the recorded run belongs to.
        #[arg(long)]
        sha: String,
    },
    /// Fail when this run breaks something the default branch does not.
    Check {
        #[command(flatten)]
        common: Common,
    },
}

#[derive(Debug, Args)]
struct Common {
    /// Directory holding this run's `JUnit` reports.
    #[arg(long, default_value = "target/junit")]
    reports: PathBuf,
    /// Lanes that failed without per-test identity, comma separated.
    #[arg(long, default_value = "")]
    failed_jobs: String,
    /// Journal kept on the executor, outliving artifact expiry.
    #[arg(long, env = "KITHARA_VERDICT_JOURNAL")]
    journal: PathBuf,
    /// Commit this run is based on. When the journal remembers that exact
    /// commit, the comparison is against what it failed rather than against
    /// the window alone.
    #[arg(long, env = "CI_MERGE_REQUEST_DIFF_BASE_SHA")]
    base: Option<String>,
}

/// One run of the default branch, as the journal remembers it.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Run {
    sha: String,
    tests: BTreeSet<String>,
    jobs: BTreeSet<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    runs: Vec<Run>,
}

impl Journal {
    fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    fn store(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self).context("serializing the verdict journal")?;
        fs::write(path, text + "\n").with_context(|| format!("writing {}", path.display()))
    }

    /// A run replaces its own earlier entry rather than stacking beside it, so
    /// a retried commit does not hold two seats in the window.
    fn record(&mut self, run: Run) {
        self.runs.retain(|kept| kept.sha != run.sha);
        self.runs.push(run);
        let excess = self.runs.len().saturating_sub(REMEMBERED_RUNS);
        self.runs.drain(..excess);
    }

    fn run_at(&self, sha: &str) -> Option<&Run> {
        self.runs.iter().find(|run| run.sha == sha)
    }

    fn tests(&self) -> BTreeSet<&str> {
        self.runs
            .iter()
            .flat_map(|run| run.tests.iter().map(String::as_str))
            .collect()
    }

    fn jobs(&self) -> BTreeSet<&str> {
        self.runs
            .iter()
            .flat_map(|run| run.jobs.iter().map(String::as_str))
            .collect()
    }
}

/// A test's identity across runs and lanes.
fn case_id(case: &CaseTiming) -> String {
    format!("{}::{}", case.suite, case.name)
}

pub(crate) fn run(args: &VerdictArgs) -> Result<()> {
    match &args.command {
        VerdictCommand::Record { common, sha } => record(common, sha),
        VerdictCommand::Check { common } => check(common),
    }
}

fn record(common: &Common, sha: &str) -> Result<()> {
    let observed = observe(common)?;
    let mut journal = Journal::load(&common.journal)?;
    journal.record(Run {
        sha: sha.to_owned(),
        tests: observed.tests.clone(),
        jobs: observed.jobs.clone(),
    });
    journal.store(&common.journal)?;
    info!(
        tests = observed.tests.len(),
        jobs = observed.jobs.len(),
        remembered = journal.runs.len(),
        "recorded what the default branch is failing"
    );
    Ok(())
}

fn check(common: &Common) -> Result<()> {
    let observed = observe(common)?;
    let journal = Journal::load(&common.journal)?;
    if journal.runs.is_empty() {
        bail!(
            "the journal at {} is empty; the default branch has to record a run before a \
             regression can be told from what it already carries",
            common.journal.display()
        );
    }
    let known_tests = journal.tests();
    let known_jobs = journal.jobs();
    let at_base = attested_at_base(&journal, common.base.as_deref());
    let new_tests: Vec<&String> = observed
        .tests
        .iter()
        .filter(|id| !known_tests.contains(id.as_str()))
        .collect();
    let new_jobs: Vec<&String> = observed
        .jobs
        .iter()
        .filter(|job| !known_jobs.contains(job.as_str()))
        .collect();
    report(
        &observed,
        &known_tests,
        &at_base,
        &new_tests,
        &new_jobs,
        &journal,
    );
    if new_tests.is_empty() && new_jobs.is_empty() {
        return Ok(());
    }
    bail!(
        "{} test(s) and {} lane(s) fail here and not on the default branch",
        new_tests.len(),
        new_jobs.len()
    )
}

/// The window is what a failure has to be new against: it unions the last runs
/// of the default branch, so a test that fails a quarter of the time does not
/// read as a regression whenever the run it is compared with happened to be
/// green.
///
/// The base commit does not widen it — that run is already one of the five —
/// but it is what makes a known failure arguable: "this was failing at the
/// commit you branched from" is evidence, "this failed sometime last week" is
/// not. Returned separately for exactly that.
fn attested_at_base<'a>(journal: &'a Journal, base: Option<&str>) -> BTreeSet<&'a str> {
    let Some(sha) = base else {
        warn!("no base commit named; a known failure cannot be attributed to one");
        return BTreeSet::new();
    };
    let Some(run) = journal.run_at(sha) else {
        warn!(
            base = %sha,
            "the journal does not remember this base commit, so nothing can be attributed to it; \
             the window still decides the verdict"
        );
        return BTreeSet::new();
    };
    info!(base = %sha, "the base commit has a recorded run");
    run.tests.iter().map(String::as_str).collect()
}

fn report(
    observed: &Observed,
    known_tests: &BTreeSet<&str>,
    at_base: &BTreeSet<&str>,
    new_tests: &[&String],
    new_jobs: &[&String],
    journal: &Journal,
) {
    report_known(observed, known_tests, at_base);
    for id in new_tests {
        warn!(test = %id, "failing here and not on the default branch");
    }
    for job in new_jobs {
        warn!(job = %job, "lane failed without per-test identity, and not on the default branch");
    }
    info!(
        cases = observed.cases,
        regressed_tests = new_tests.len(),
        regressed_jobs = new_jobs.len(),
        attested_at_base = at_base.len(),
        remembered_runs = journal.runs.len(),
        "verdict"
    );
}

fn report_known(observed: &Observed, known_tests: &BTreeSet<&str>, at_base: &BTreeSet<&str>) {
    for id in observed
        .tests
        .iter()
        .filter(|id| known_tests.contains(id.as_str()))
    {
        if at_base.contains(id.as_str()) {
            warn!(test = %id, "failing, and failing at the base commit too");
        } else {
            warn!(test = %id, "failing, and seen failing on the default branch recently");
        }
    }
}

struct Observed {
    tests: BTreeSet<String>,
    jobs: BTreeSet<String>,
    cases: usize,
}

/// Every lane that can name its tests contributes them; a lane that cannot —
/// the browser suites, the sanitiser runs, mutation — contributes its own name
/// instead, so a failure there is still something a merge request can be held
/// on rather than a silence.
fn observe(common: &Common) -> Result<Observed> {
    let cases = collect(&common.reports)?;
    if cases.is_empty() && common.failed_jobs.trim().is_empty() {
        bail!(
            "no JUnit reports under {} and no failed lanes named — a verdict on nothing is not a \
             verdict",
            common.reports.display()
        );
    }
    Ok(Observed {
        tests: cases
            .iter()
            .filter(|case| case.failed)
            .map(case_id)
            .collect(),
        jobs: common
            .failed_jobs
            .split(',')
            .map(str::trim)
            .filter(|job| !job.is_empty())
            .map(str::to_owned)
            .collect(),
        cases: cases.len(),
    })
}

fn collect(root: &Path) -> Result<Vec<CaseTiming>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut cases = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            let entries =
                fs::read_dir(&path).with_context(|| format!("reading {}", path.display()))?;
            for entry in entries {
                stack.push(entry.context("reading a report directory entry")?.path());
            }
        } else if path.extension().is_some_and(|kind| kind == "xml") {
            let text =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            cases
                .extend(parse_junit(&text).with_context(|| format!("parsing {}", path.display()))?);
        }
    }
    Ok(cases)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_of(sha: &str, tests: &[&str], jobs: &[&str]) -> Run {
        Run {
            sha: sha.to_owned(),
            tests: tests.iter().map(|id| (*id).to_owned()).collect(),
            jobs: jobs.iter().map(|id| (*id).to_owned()).collect(),
        }
    }

    #[test]
    fn the_window_unions_every_run_it_remembers() {
        let mut journal = Journal::default();
        journal.record(run_of("one", &["suite::a"], &[]));
        journal.record(run_of("two", &["suite::b"], &["web:firefox"]));
        assert_eq!(journal.tests(), BTreeSet::from(["suite::a", "suite::b"]));
        assert_eq!(journal.jobs(), BTreeSet::from(["web:firefox"]));
    }

    #[test]
    fn a_retried_commit_does_not_hold_two_seats() {
        let mut journal = Journal::default();
        journal.record(run_of("one", &["suite::a"], &[]));
        journal.record(run_of("one", &["suite::b"], &[]));
        assert_eq!(journal.runs.len(), 1);
        assert_eq!(journal.tests(), BTreeSet::from(["suite::b"]));
    }

    #[test]
    fn the_window_forgets_what_falls_out_of_it() {
        let mut journal = Journal::default();
        for index in 0..=REMEMBERED_RUNS {
            journal.record(run_of(
                &index.to_string(),
                &[&format!("suite::{index}")],
                &[],
            ));
        }
        assert_eq!(journal.runs.len(), REMEMBERED_RUNS);
        assert!(!journal.tests().contains("suite::0"));
        assert!(journal.tests().contains("suite::5"));
    }

    #[test]
    fn the_journal_can_answer_for_one_commit() {
        let mut journal = Journal::default();
        journal.record(run_of("base", &["suite::a"], &[]));
        journal.record(run_of("later", &["suite::b"], &[]));
        assert_eq!(
            journal.run_at("base").map(|run| run.tests.clone()),
            Some(BTreeSet::from(["suite::a".to_owned()]))
        );
        assert!(journal.run_at("absent").is_none());
    }

    #[test]
    fn a_failure_is_attributed_to_the_base_commit_when_the_journal_has_it() {
        let mut journal = Journal::default();
        journal.record(run_of("base", &["suite::a"], &[]));
        journal.record(run_of("later", &["suite::b"], &[]));
        assert_eq!(
            attested_at_base(&journal, Some("base")),
            BTreeSet::from(["suite::a"])
        );
    }

    #[test]
    fn an_unremembered_base_attributes_nothing_and_still_lets_the_window_decide() {
        let mut journal = Journal::default();
        journal.record(run_of("later", &["suite::b"], &[]));
        assert!(attested_at_base(&journal, Some("gone")).is_empty());
        assert!(attested_at_base(&journal, None).is_empty());
        // The window is untouched by either: it is what the verdict compares to.
        assert_eq!(journal.tests(), BTreeSet::from(["suite::b"]));
    }

    #[test]
    fn a_journal_round_trips_through_the_executor() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state/regressions.json");
        let mut journal = Journal::default();
        journal.record(run_of("one", &["suite::a"], &["deep:rtsan"]));
        journal.store(&path).unwrap();

        let loaded = Journal::load(&path).unwrap();
        assert_eq!(loaded.tests(), BTreeSet::from(["suite::a"]));
        assert_eq!(loaded.jobs(), BTreeSet::from(["deep:rtsan"]));
    }

    #[test]
    fn a_missing_journal_reads_as_empty_rather_than_failing() {
        let directory = tempfile::tempdir().unwrap();
        let journal = Journal::load(&directory.path().join("absent.json")).unwrap();
        assert!(journal.runs.is_empty());
    }
}
