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

use super::{config::CiConfig, run::PipelineKind};

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

/// Where the verdict expects every lane to leave what it produced. Artifacts
/// travel between runners at their own paths, so one directory is what makes a
/// report from the simulator and a report from a container comparable.
pub(crate) const REPORT_DIR: &str = "target/junit";

/// The report a lane writes, before it is collected. A lane that writes none
/// still leaves a marker when it fails, which is how a browser suite or a
/// sanitiser run — neither of which can name a test — still holds a merge
/// request.
pub(crate) fn produced_report(lane: &str) -> Option<&'static str> {
    match lane {
        "apple-test" | "linux-test" | "windows-arm64" | "windows-x64" => {
            Some("target/nextest/ci/junit.xml")
        }
        "apple-test-flash-off" => Some("target-flash-off/nextest/ci/junit.xml"),
        "apple-ios-test" => Some("target/xcresult/ios-test.junit.xml"),
        "apple-swift-test" => Some("target/xcresult/swift-test.junit.xml"),
        _ => None,
    }
}

/// The build directory survives between jobs on this executor, so a report left
/// by an earlier lane would be collected as this one's. Remove it first: a lane
/// that produces nothing must publish nothing.
pub(crate) fn clear(root: &Path, lane: &str) -> Result<()> {
    let Some(report) = produced_report(lane) else {
        return Ok(());
    };
    let path = root.join(report);
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

/// Collect what this lane produced into the one directory the verdict reads,
/// and record a bare failure for a lane that cannot name its tests.
pub(crate) fn gather(root: &Path, lane: &str, failed: bool) -> Result<()> {
    let directory = root.join(REPORT_DIR);
    fs::create_dir_all(&directory).with_context(|| format!("creating {}", directory.display()))?;
    if let Some(report) = produced_report(lane) {
        let from = root.join(report);
        if from.exists() {
            let to = directory.join(format!("{lane}.xml"));
            fs::copy(&from, &to)
                .with_context(|| format!("copying {} to {}", from.display(), to.display()))?;
        }
    }
    if failed {
        let marker = directory.join(format!("{lane}.failed"));
        fs::write(&marker, "").with_context(|| format!("writing {}", marker.display()))?;
    }
    Ok(())
}

/// The journal lives on the executor that runs this lane. The Linux container
/// is the one with a path that outlives a job — a macOS job runs in a throwaway
/// guest, where nothing written survives it.
fn journal_path(config: &CiConfig) -> PathBuf {
    config.host.cache_root_linux.join("verdict/journal.json")
}

/// `main` and the nightly chain describe the default branch, so they record.
/// Everything else is measured against what they recorded.
pub(crate) fn lane(root: &Path, config: &CiConfig, kind: PipelineKind) -> Result<()> {
    let common = Common {
        reports: root.join(REPORT_DIR),
        journal: journal_path(config),
        base: std::env::var("CI_MERGE_REQUEST_DIFF_BASE_SHA").ok(),
    };
    match kind {
        PipelineKind::Main | PipelineKind::Nightly => {
            let sha = std::env::var("CI_COMMIT_SHA")
                .context("CI_COMMIT_SHA names the run being recorded")?;
            record(&common, &sha)
        }
        PipelineKind::Branch
        | PipelineKind::MergeRequest
        | PipelineKind::Quarantine
        | PipelineKind::Weekly
        | PipelineKind::Release => check(&common),
    }
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
    let jobs = failed_markers(&common.reports)?;
    if cases.is_empty() && jobs.is_empty() {
        bail!(
            "no `JUnit` reports and no failure markers under {} — a verdict on nothing is not a \
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
        jobs,
        cases: cases.len(),
    })
}

/// A lane that failed leaves its name behind, whether or not it could name a
/// test. `GitLab` hands a job no status for the jobs it needed, so the marker is
/// what carries that across.
fn failed_markers(root: &Path) -> Result<BTreeSet<String>> {
    if !root.exists() {
        return Ok(BTreeSet::new());
    }
    let mut jobs = BTreeSet::new();
    let entries = fs::read_dir(root).with_context(|| format!("reading {}", root.display()))?;
    for entry in entries {
        let path = entry.context("reading a report directory entry")?.path();
        if path.extension().is_some_and(|kind| kind == "failed")
            && let Some(lane) = path.file_stem().and_then(|stem| stem.to_str())
        {
            jobs.insert(lane.to_owned());
        }
    }
    Ok(jobs)
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
    fn a_lane_that_produced_nothing_publishes_nothing() {
        let directory = tempfile::tempdir().unwrap();
        gather(directory.path(), "apple-lint", false).unwrap();
        assert!(directory.path().join(REPORT_DIR).exists());
        assert!(
            fs::read_dir(directory.path().join(REPORT_DIR))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[test]
    fn a_failing_lane_leaves_its_name_even_without_a_report() {
        let directory = tempfile::tempdir().unwrap();
        gather(directory.path(), "web-chromium", true).unwrap();
        assert_eq!(
            failed_markers(&directory.path().join(REPORT_DIR)).unwrap(),
            BTreeSet::from(["web-chromium".to_owned()])
        );
    }

    #[test]
    fn a_report_left_by_an_earlier_job_is_not_collected_as_this_ones() {
        let directory = tempfile::tempdir().unwrap();
        let stale = directory.path().join("target/nextest/ci/junit.xml");
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(&stale, "<testsuites/>").unwrap();

        clear(directory.path(), "apple-test").unwrap();
        gather(directory.path(), "apple-test", false).unwrap();
        assert!(
            !directory
                .path()
                .join(REPORT_DIR)
                .join("apple-test.xml")
                .exists(),
            "the build directory survives between jobs, so a stale report must not travel"
        );
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
