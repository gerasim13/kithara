//! Portable repeated-test campaign and independent evidence verification.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs,
    io::Write as _,
    path::{Component, Path, PathBuf},
    process::{Command, ExitStatus},
};

use anyhow::{Context, Error, Result, ensure};
use clap::{Args, Subcommand};

/// Bounds the distinct sanitizer findings one lane section lists.
const MAX_FINDING_ROWS: usize = 100;

use crate::{
    Ctx,
    common::project::{
        ProjectConfig, StressArtifactConfig, StressConfig, StressEvidenceConfig, StressModeConfig,
    },
    stress_report::{self, StressReportArgs},
    stress_run::{self, StressRunSpec},
    test::{ConfiguredLane, configured_lane},
    verdict::{ChildFailure, NotClean},
};

mod environment;
mod manifest;
mod output;
pub(crate) mod pressure;
mod system;

use environment::CampaignEnvironment;
use manifest::{
    ExecuteResult, ExpectedProvenance, Manifest, ManifestConfig, ManifestSpec, PolicySnapshot,
    Selection,
};
use pressure::Sampler;

#[derive(Debug, Subcommand)]
#[non_exhaustive]
pub enum StressCommand {
    /// Run the complete repeated-test campaign and preserve its evidence.
    Run(RunArgs),
    /// Independently verify and render a downloaded campaign artifact.
    Report(ReportArgs),
}

#[derive(Debug, Args)]
#[non_exhaustive]
pub struct RunArgs {
    /// Subject workspace whose tests are selected and executed.
    #[arg(long, default_value = ".")]
    subject_root: PathBuf,
    /// Fresh raw evidence directory owned by this campaign.
    #[arg(long)]
    output: Option<PathBuf>,
    /// Nextest filterset selecting tests to repeat.
    #[arg(long)]
    filter: Option<String>,
    /// Number of times to run every selected test.
    #[arg(long)]
    count: Option<usize>,
    /// Configured campaign mode; repeat to run several, empty for the
    /// project's own list. Each becomes one lane of the same campaign.
    #[arg(long = "mode")]
    modes: Vec<String>,
    /// Trusted controller revision to compare with the checkout.
    #[arg(long)]
    expected_controller_sha: Option<String>,
    /// Trusted subject revision to compare with the checkout.
    #[arg(long)]
    expected_subject_sha: Option<String>,
}

#[derive(Debug, Args)]
#[non_exhaustive]
pub struct ReportArgs {
    /// Downloaded raw evidence directory.
    #[arg(long)]
    raw: PathBuf,
    /// Markdown report destination.
    #[arg(long)]
    output: Option<PathBuf>,
    #[arg(long)]
    expected_controller_sha: String,
    #[arg(long)]
    expected_subject_sha: String,
    #[arg(long)]
    filter: Option<String>,
    #[arg(long)]
    count: Option<usize>,
    /// Lane to verify; repeat to verify several, empty for the project's list.
    #[arg(long = "mode")]
    modes: Vec<String>,
    #[arg(long)]
    execute_result: ExecuteResult,
}

/// Runs the selected stress command.
///
/// # Errors
///
/// Returns an error when execution, finalization, or verification fails.
pub(crate) fn run(command: &StressCommand, ctx: &Ctx) -> Result<()> {
    match command {
        StressCommand::Run(args) => run_campaign(args, ctx),
        StressCommand::Report(args) => run_report(args, ctx),
    }
}

pub(crate) fn run_output(command: &mut Command, path: &Path) -> Result<ExitStatus> {
    output::run(command, path)
}

pub(crate) fn run_stderr_output(command: &mut Command, path: &Path) -> Result<ExitStatus> {
    output::run_stderr(command, path)
}

#[derive(Debug)]
struct Paths {
    raw: PathBuf,
    attempts: PathBuf,
    /// One `JUnit` copy per attempt of a command lane, named by attempt.
    attempt_junit: PathBuf,
    envelopes: Option<PathBuf>,
    inventory: PathBuf,
    junit: PathBuf,
    log: PathBuf,
    manifest: PathBuf,
    lines: Option<PathBuf>,
    pressure: PathBuf,
    report: PathBuf,
}

struct ReportExpectation<'a> {
    config: &'a StressConfig,
    mode_name: &'a str,
    mode: &'a StressModeConfig,
    filter: &'a str,
    count: usize,
    runner: ConfiguredLane,
}

impl<'a> ReportExpectation<'a> {
    /// Derives what the lane should have been from the same place the lane
    /// itself did.
    ///
    /// The runner is not passed in. A lane records the runner `lane_runner`
    /// gave it, so anything else the report expects is a second opinion about
    /// the same question, and the two disagree exactly where the lanes differ
    /// most — a command lane runs its own command and would read as evidence
    /// of unknown origin against the test runner's identity.
    fn new(
        project: &ProjectConfig,
        config: &'a StressConfig,
        mode_name: &'a str,
        mode: &'a StressModeConfig,
        filter: &'a str,
        count: usize,
    ) -> Result<Self> {
        Ok(Self {
            config,
            mode_name,
            mode,
            filter,
            count,
            runner: lane_runner(project, config, mode)?,
        })
    }
}

impl Paths {
    fn new(raw: PathBuf, artifacts: &StressArtifactConfig) -> Self {
        Self {
            attempts: raw.join(&artifacts.attempts),
            attempt_junit: raw.join("attempt-junit"),
            envelopes: artifacts.envelope_dir.as_deref().map(|path| raw.join(path)),
            inventory: raw.join(&artifacts.inventory),
            junit: raw.join(&artifacts.junit),
            log: raw.join(&artifacts.log),
            manifest: raw.join(&artifacts.manifest),
            lines: artifacts.line_log.as_deref().map(|path| raw.join(path)),
            pressure: raw.join(&artifacts.pressure),
            report: raw.join(&artifacts.report),
            raw,
        }
    }
}

/// Runs every lane of one campaign, in order, into one evidence directory.
///
/// A lane that fails does not stop the ones after it. A campaign exists to
/// find out which lane a flake belongs to, and a run that stopped at the first
/// red lane would answer that question only when the answer was already known.
/// The first failure is what the campaign returns, after all of them have run.
fn run_campaign(args: &RunArgs, ctx: &Ctx) -> Result<()> {
    let config = &ctx.config.stress;
    ensure!(config.is_configured(), "stress campaign is not configured");
    let lanes = campaign_lanes(&args.modes, config)?;
    let root = absolute_from(
        &ctx.root,
        args.output
            .as_deref()
            .unwrap_or_else(|| Path::new(&config.raw_output)),
    );
    let subject_root = absolute_existing_directory(&ctx.root, &args.subject_root, "subject")?;
    let subject_junit = subject_root.join(&config.artifacts.subject_junit);
    ensure!(
        !subject_junit
            .try_exists()
            .with_context(|| format!("inspect stress JUnit path {}", subject_junit.display()))?,
        "stress JUnit already exists: {}; remove it before starting a new campaign",
        subject_junit.display()
    );
    prepare_campaign_root(&root)?;
    let mut failure = None;
    for lane in &lanes {
        let outcome = run_lane(args, ctx, lane, &root.join(lane));
        if let Err(error) = outcome
            && failure.is_none()
        {
            failure = Some(error);
        }
    }
    failure.map_or(Ok(()), Err)
}

/// The lanes this invocation is made of: what was asked for, or what the
/// project says a campaign is.
fn campaign_lanes(requested: &[String], config: &StressConfig) -> Result<Vec<String>> {
    let lanes = if requested.is_empty() {
        config.default_modes.clone()
    } else {
        requested.to_vec()
    };
    ensure!(!lanes.is_empty(), "a campaign must name at least one mode");
    let mut seen = BTreeSet::new();
    for lane in &lanes {
        config.mode(lane)?;
        validate_lane_directory(lane)?;
        ensure!(seen.insert(lane), "campaign mode `{lane}` is named twice");
    }
    Ok(lanes)
}

/// A lane names the directory its evidence lands in, so it has to be a plain
/// directory name rather than anything that could climb out of the campaign.
fn validate_lane_directory(lane: &str) -> Result<()> {
    let mut components = Path::new(lane).components();
    let single =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    ensure!(
        single && !lane.is_empty(),
        "campaign mode `{lane}` is not usable as a directory name"
    );
    Ok(())
}

/// What a lane actually invokes, in the shape the manifest records.
///
/// A command lane is described by its own words rather than by the project's
/// test runner, so that its manifest names what really ran and the reporter
/// can verify it the same way it verifies any other lane.
fn lane_runner(
    project: &ProjectConfig,
    config: &StressConfig,
    mode: &StressModeConfig,
) -> Result<ConfiguredLane> {
    let Some((program, arguments)) = mode.command.split_first() else {
        return configured_lane(project, &config.lane, &config.backend, &mode.features);
    };
    Ok(ConfiguredLane {
        lane: config.lane.clone(),
        backend: config.backend.clone(),
        program: program.clone(),
        prefix_args: arguments.to_vec(),
        suffix_args: Vec::new(),
        feature_arg: "--features".to_owned(),
        features: Vec::new(),
    })
}

/// Records one exit code per attempt, in order.
/// Separates one attempt's output from the next in the lane's shared log.
///
/// The attempts append to one file, so without a boundary a finding cannot be
/// told apart from the same finding on a later attempt — and a violation that
/// fires once in fifty would be indistinguishable from one that fires always.
fn mark_attempt(log: &Path, attempt: usize) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .with_context(|| format!("open stress log {}", log.display()))?;
    writeln!(file, "{}{attempt}", stress_report::ATTEMPT_MARKER)
        .with_context(|| format!("write stress log {}", log.display()))
}

fn write_attempts(path: &Path, codes: &[i32]) -> Result<()> {
    let json = serde_json::to_string(codes).context("serialize command lane attempts")?;
    fs::write(path, json).with_context(|| format!("write command lane attempts {}", path.display()))
}

/// Repeats a lane's own command and records what each attempt did.
///
/// A sanitizer aborts the process at the offending call, so there is no
/// per-test verdict to collect — the evidence is an exit code per attempt and
/// the log the attempt wrote. Repetition is the point: a violation that fires
/// on one attempt in two is a defect with a rate, and a single green run has
/// never been evidence that it is gone.
fn run_command_lane(
    ctx: &Ctx,
    mode: &StressModeConfig,
    paths: &Paths,
    count: usize,
    environment: &CampaignEnvironment,
) -> Result<Vec<i32>> {
    let (program, arguments) = mode
        .command
        .split_first()
        .context("a command lane needs a program to run")?;
    let report = mode
        .attempt_junit
        .as_deref()
        .map(|path| ctx.root.join(path));
    let mut codes = Vec::with_capacity(count);
    for attempt in 0..count {
        mark_attempt(&paths.log, attempt)?;
        // The runner writes to one path. Removing it first means a copy can
        // only ever be this attempt's: an attempt that died before writing
        // leaves no file rather than the previous attempt's verdict under a
        // new number.
        if let Some(report) = report.as_deref()
            && let Err(error) = fs::remove_file(report)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error)
                .with_context(|| format!("clear command lane report {}", report.display()));
        }
        let mut command = Command::new(program);
        command.args(arguments).current_dir(&ctx.root);
        environment.apply(&mut command);
        let status = run_output(&mut command, &paths.log)?;
        let code = status.code().unwrap_or_else(|| i32::from(u8::MAX));
        codes.push(code);
        if let Some(report) = report.as_deref() {
            keep_attempt_report(report, &paths.attempt_junit, attempt)?;
        }
    }
    Ok(codes)
}

/// Keeps this attempt's report, if the command left one.
///
/// An aborted attempt may leave nothing at all — that absence is itself
/// evidence, and it is recorded by the copy that is missing rather than by an
/// error here.
fn keep_attempt_report(report: &Path, directory: &Path, attempt: usize) -> Result<()> {
    if !report.is_file() {
        return Ok(());
    }
    fs::create_dir_all(directory)
        .with_context(|| format!("create attempt report directory {}", directory.display()))?;
    let kept = directory.join(format!("attempt-{attempt}.xml"));
    fs::copy(report, &kept)
        .with_context(|| format!("keep attempt report {}", kept.display()))
        .map(|_| ())
}

fn run_lane(args: &RunArgs, ctx: &Ctx, mode_name: &str, raw: &Path) -> Result<()> {
    let config = &ctx.config.stress;
    let mode = config.mode(mode_name)?;
    let filter = args
        .filter
        .clone()
        .unwrap_or_else(|| config.default_filter.clone());
    let count = args.count.unwrap_or(config.default_count);
    validate_count(count, config.max_count)?;
    let subject_root = absolute_existing_directory(&ctx.root, &args.subject_root, "subject")?;
    let paths = Paths::new(raw.to_path_buf(), &config.artifacts);
    let config_file = absolute_existing_file(
        &ctx.root,
        Path::new(&config.nextest_config),
        "nextest config",
    )?;
    let controller_sha = revision(
        &ctx.root,
        args.expected_controller_sha.as_deref(),
        "controller",
    )?;
    let subject_sha = revision(
        &subject_root,
        args.expected_subject_sha.as_deref(),
        "subject",
    )?;
    let subject_junit = subject_root.join(&config.artifacts.subject_junit);
    let runner = lane_runner(&ctx.config, config, mode)?;
    let commanded = !mode.command.is_empty();
    let spec = StressRunSpec {
        inventory: paths.inventory.clone(),
        junit: subject_junit.clone(),
        config_file: config_file.clone(),
        filter: filter.clone(),
        count,
        test_threads: config.test_threads.clone(),
        profile: config.nextest_profile.clone(),
        max_count: config.max_count,
        max_test_threads: config.max_test_threads,
        runner: runner.clone(),
    };
    if !commanded {
        stress_run::validate(&spec)?;
        clear_previous_lane_junit(&subject_junit)?;
    }
    ensure_raw_outside_subject_evidence(&paths.raw, &subject_junit)?;
    let system = system::capture()?;
    let environment = CampaignEnvironment::new(&paths.raw, config, mode)?;
    let mut manifest = Manifest::start(
        ManifestSpec {
            mode: mode_name.to_owned(),
            config: ManifestConfig::new(
                config.nextest_profile.clone(),
                config.nextest_config.clone(),
                config.workflow_job_timeout_minutes,
            ),
            controller_sha,
            subject_sha,
            runner,
            selection: Selection {
                filter: filter.clone(),
                count,
                test_threads: config.test_threads.clone(),
            },
            policy: policy_snapshot(config, mode),
        },
        system.clone(),
    )?;
    prepare_raw_directory(&paths)?;
    let manifest_start = manifest.write_atomic(&paths.manifest);
    let sampler = Sampler::start(
        &paths.pressure,
        system.cgroup_v2.path.as_deref(),
        system.cgroup_v2.scope.as_str(),
    );

    let (primary, sampler_result) = match (manifest_start, sampler) {
        (Ok(()), Ok(sampler)) => {
            let primary = if commanded {
                run_command_lane(ctx, mode, &paths, count, &environment).and_then(|codes| {
                    write_attempts(&paths.attempts, &codes)?;
                    let failed = codes.iter().filter(|code| **code != 0).count();
                    if failed == 0 {
                        Ok(())
                    } else {
                        Err(ChildFailure::inherited(
                            format!("{failed} of {count} attempts"),
                            codes.iter().copied().find(|code| *code != 0),
                        ))
                    }
                })
            } else {
                stress_run::run(&spec, &subject_root, &paths.log, &|command| {
                    environment.apply(command);
                })
            };
            let primary_code = result_code(&primary);
            let sampler_result = sampler.finish(Some(primary_code));
            (primary, sampler_result)
        }
        (Err(error), Ok(sampler)) => {
            let primary = Err(error);
            let sampler_result = sampler.finish(None);
            (primary, sampler_result)
        }
        (Ok(()), Err(error)) => (Ok(()), Err(error)),
        (Err(manifest_error), Err(sampler_error)) => (Err(manifest_error), Err(sampler_error)),
    };
    let sampler_healthy = sampler_result.is_ok();
    // A command lane produces no per-test evidence, so there is nothing to
    // stage and nothing for the per-test reporter to read. Its verdict is the
    // attempts it recorded.
    let (stage_result, report_result) = if commanded {
        (Ok(()), Ok(()))
    } else {
        (
            stage_junit(&subject_junit, &paths.junit),
            render_raw_report(&paths, count, config),
        )
    };
    let final_error = choose_failure(
        primary,
        sampler_result.map(|_| ()),
        stage_result,
        report_result,
    );
    let final_code = final_error.as_ref().map_or(0, error_code);
    manifest.finalize(final_code, sampler_healthy)?;
    manifest.write_atomic(&paths.manifest)?;
    final_error.map_or(Ok(()), Err)
}

fn policy_snapshot(config: &StressConfig, mode: &StressModeConfig) -> PolicySnapshot {
    PolicySnapshot {
        features: mode.features.clone(),
        remove_env: config.environment.remove.clone(),
        set_env: mode.set_env.clone(),
        raw_path_env: mode.raw_path_env.clone(),
        evidence: config.evidence.clone(),
    }
}

fn render_raw_report(paths: &Paths, count: usize, config: &StressConfig) -> Result<()> {
    let args = StressReportArgs::new(
        paths.junit.clone(),
        paths.inventory.clone(),
        paths.report.clone(),
        count,
    )
    .with_evidence(config.evidence.clone())
    .with_pressure(paths.pressure.clone())
    .with_optional_envelopes(
        paths
            .envelopes
            .as_ref()
            .filter(|path| path.is_dir())
            .cloned(),
    )
    .with_optional_lines(paths.lines.clone());
    stress_report::run(&args)
}

/// Verifies every lane of a downloaded campaign and renders them as one report.
///
/// The lanes are read independently — each carries its own manifest, inventory
/// and `JUnit`, and each is checked against what the project says it should have
/// been. They are rendered together because the question a multi-lane campaign
/// answers is a comparison, and a comparison split across two documents is one
/// the reader has to make by hand.
fn run_report(args: &ReportArgs, ctx: &Ctx) -> Result<()> {
    let config = &ctx.config.stress;
    ensure!(config.is_configured(), "stress campaign is not configured");
    let lanes = campaign_lanes(&args.modes, config)?;
    let filter = args
        .filter
        .clone()
        .unwrap_or_else(|| config.default_filter.clone());
    let count = args.count.unwrap_or(config.default_count);
    validate_count(count, config.max_count)?;
    let raw_root = absolute_from_current(&args.raw)?;
    let output = absolute_from(
        &ctx.root,
        args.output
            .as_deref()
            .unwrap_or_else(|| Path::new(&config.report_output)),
    );
    ensure_report_outside_raw(&raw_root, &output)?;

    let mut sections = String::new();
    let mut measured = Vec::new();
    let mut commanded = Vec::new();
    let mut excluded = Vec::new();
    let mut exit_codes = Vec::new();
    let mut failure = None;
    for lane_name in &lanes {
        let mode = config.mode(lane_name)?;
        let paths = Paths::new(raw_root.join(lane_name), &config.artifacts);
        let report_args = StressReportArgs::new(
            paths.junit.clone(),
            paths.inventory.clone(),
            output.clone(),
            count,
        )
        .with_allow_missing(true)
        .with_evidence(config.evidence.clone())
        .with_pressure(paths.pressure.clone())
        .with_optional_envelopes(
            paths
                .envelopes
                .as_ref()
                .filter(|path| path.is_dir())
                .cloned(),
        )
        .with_optional_lines(paths.lines.clone());
        let lane = if mode.command.is_empty() {
            stress_report::lane_report(&report_args)?
        } else {
            command_lane_report(&paths, count, &config.evidence)
        };
        let expectation =
            ReportExpectation::new(&ctx.config, config, lane_name, mode, &filter, count)?;
        let checked = verify_manifest(args, &expectation, &paths.manifest);
        let trusted = checked.verdict.is_ok();
        let excluded_because = exclusion_reason(trusted, &lane);
        exit_codes.push(checked.exit_code);
        let body = with_provenance(lane.markdown, &checked.verdict, &checked.details)?;
        writeln!(sections, "\n# Lane `{}`\n", markdown_cell(lane_name))?;
        sections.push_str(&body);
        // Only a lane that verified against its expected identity, read valid
        // evidence, AND accounted for every requested iteration may stand in a
        // comparison. Numbers from one that did not are of unknown origin, and
        // putting them beside trustworthy ones is how a campaign reports a
        // difference between lanes that is really a difference between runs.
        // A lane short of its own request measures a smaller campaign than the
        // one that was asked for, so its rate belongs to a different question.
        match excluded_because {
            Some(reason) => excluded.push((lane_name.clone(), reason)),
            None => match lane.attempts {
                Some(rate) => commanded.push((lane_name.clone(), rate)),
                None => measured.push((lane_name.clone(), lane.rates)),
            },
        }
        let lane_failure = choose_failure(lane.verdict, checked.verdict, Ok(()), Ok(()));
        if let Some(error) = lane_failure
            && failure.is_none()
        {
            failure = Some(error);
        }
    }

    let campaign = verify_campaign_result(args.execute_result, &exit_codes);
    let mut document =
        stress_report::render_lane_comparison(&measured, &commanded, &excluded, lanes.len());
    if let Err(error) = &campaign {
        let _ = writeln!(
            document,
            "\n- Campaign provenance: `{}`",
            markdown_cell(&format!("{error:#}"))
        );
    }
    document.push_str(&sections);
    stress_report::write_report(&output, &document)?;
    choose_failure(failure.map_or(Ok(()), Err), campaign, Ok(()), Ok(())).map_or(Ok(()), Err)
}

/// Reads what a command lane recorded and states it as a rate.
///
/// The only thing such a lane can say is how many of its attempts the command
/// rejected. That is exactly the number a one-shot gate cannot produce: a
/// sanitizer that aborts on one attempt in two is green half the time, and
/// half the time is what has kept its defect open.
/// Why this lane may not stand beside the others, or `None` when it may.
///
/// Named rather than counted: "one lane was dropped" sends the reader back to
/// the per-lane sections to work out which one and why, and that is the join
/// the campaign document exists to spare them.
fn exclusion_reason(trusted: bool, lane: &stress_report::LaneReport) -> Option<String> {
    if !trusted {
        return Some("failed provenance against its expected identity".to_owned());
    }
    if !lane.readable {
        return Some("evidence artifact missing or invalid".to_owned());
    }
    if !lane.complete {
        return Some("incomplete evidence: fewer iterations than requested".to_owned());
    }
    None
}

fn command_lane_report(
    paths: &Paths,
    expected: usize,
    evidence: &StressEvidenceConfig,
) -> stress_report::LaneReport {
    let attempts = &paths.attempts;
    let log = &paths.log;
    let mut markdown = String::from("# Stress evidence\n");
    let codes = match fs::read_to_string(attempts)
        .with_context(|| format!("read command lane attempts {}", attempts.display()))
        .and_then(|text| {
            serde_json::from_str::<Vec<i32>>(&text).context("parse command lane attempts")
        }) {
        Ok(codes) => codes,
        Err(error) => {
            let _ = writeln!(
                markdown,
                "\n- Result: **NO ATTEMPTS**\n\n`{}`\n",
                markdown_cell(&format!("{error:#}"))
            );
            return stress_report::LaneReport {
                markdown,
                rates: BTreeMap::new(),
                attempts: None,
                verdict: Err(NotClean::reported("stress evidence")),
                readable: false,
                complete: false,
            };
        }
    };
    let failed = codes.iter().filter(|code| **code != 0).count();
    let observed = codes.len();
    let result = if observed != expected {
        "INCOMPLETE"
    } else if failed > 0 {
        "FAILED"
    } else {
        "PASSED"
    };
    let _ = writeln!(markdown, "\n- Result: **{result}**");
    let _ = writeln!(markdown, "- Requested attempts: `{expected}`");
    let _ = writeln!(markdown, "- Observed attempts: `{observed}`");
    let _ = writeln!(markdown, "- Rejected attempts: `{failed}`");
    if failed > 0 {
        let codes = codes
            .iter()
            .enumerate()
            .filter(|(_, code)| **code != 0)
            .map(|(attempt, code)| format!("{attempt}:{code}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            markdown,
            "- Rejected attempt:code — `{}`",
            markdown_cell(&codes)
        );
    }
    stress_report::append_attempt_reports(&mut markdown, &paths.attempt_junit, &codes);
    append_findings(&mut markdown, log, evidence, failed, observed);
    let verdict = if result == "PASSED" {
        Ok(())
    } else {
        Err(NotClean::reported("stress evidence"))
    };
    stress_report::LaneReport {
        markdown,
        rates: BTreeMap::new(),
        attempts: Some(stress_report::LaneRate {
            failed,
            attempts: observed,
        }),
        verdict,
        readable: true,
        complete: observed == expected,
    }
}

/// Reports what the sanitizer itself said, and says so when it said nothing.
///
/// A rejected attempt with no finding is not a smaller version of a finding: it
/// means the command failed for a reason this report cannot name, and printing
/// only a count there would read as though the cause had been located.
fn append_findings(
    markdown: &mut String,
    log: &Path,
    evidence: &StressEvidenceConfig,
    failed: usize,
    observed: usize,
) {
    let text = match stress_report::read_bounded_utf8(
        log,
        stress_report::MAX_LANE_LOG_BYTES,
        "stress lane log",
    ) {
        Ok(text) => text,
        Err(error) => {
            let _ = writeln!(
                markdown,
                "\nEvidence problem: this lane's log could not be read, so its findings are \
                 unknown — `{}`",
                markdown_cell(&format!("{error:#}"))
            );
            return;
        }
    };
    let findings = stress_report::sanitizer_findings(&text, evidence);
    if findings.is_empty() {
        if failed > 0 {
            let _ = writeln!(
                markdown,
                "\nNo sanitizer report was found in this lane's log. The rejected attempts failed \
                 for a reason this report cannot name; the command's own output is in the log."
            );
        }
        return;
    }
    let _ = writeln!(
        markdown,
        "\n## Sanitizer findings\n\nEach names the violated contract, the call that violated it, \
         and the first project frames that reached it.\n\n\
         | finding | attempts | rate |\n|---|---|---:|"
    );
    for (signature, attempts) in findings.iter().take(MAX_FINDING_ROWS) {
        let listed = attempts
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            markdown,
            "| `{}` | {} | {} |",
            markdown_cell(signature),
            markdown_cell(&listed),
            stress_report::rate_percent(attempts.len(), observed)
        );
    }
    if findings.len() > MAX_FINDING_ROWS {
        let _ = writeln!(
            markdown,
            "\nShowing the first {MAX_FINDING_ROWS} of {} distinct findings.",
            findings.len()
        );
    }
}

/// Checks the job's own result against the campaign as a whole.
///
/// A red job means some lane failed, and which one is a fact about the
/// campaign rather than about any single manifest. Checking it per lane would
/// call every lane that passed a liar; checking it here catches the case that
/// actually matters — a job reported as failed whose lanes all say they
/// succeeded, which means the failure came from somewhere the evidence does
/// not cover.
fn verify_campaign_result(execute_result: ExecuteResult, exit_codes: &[Option<i32>]) -> Result<()> {
    if matches!(execute_result, ExecuteResult::Success) {
        return Ok(());
    }
    ensure!(
        exit_codes
            .iter()
            .any(|code| code.is_none_or(|code| code != 0)),
        "execute reported {} while every lane finished cleanly",
        execute_result.as_str()
    );
    Ok(())
}

/// What one lane's manifest says about itself, and whether it is believable.
struct LaneProvenance {
    verdict: Result<()>,
    details: Vec<String>,
    /// The lane's own exit code, kept so the campaign can check the job's
    /// result against all of its lanes rather than against each one alone.
    exit_code: Option<i32>,
}

fn verify_manifest(
    args: &ReportArgs,
    expected: &ReportExpectation<'_>,
    path: &Path,
) -> LaneProvenance {
    let manifest = match Manifest::read(path) {
        Ok(manifest) => manifest,
        Err(error) => {
            let detail = format!("{error:#}");
            return LaneProvenance {
                verdict: Err(error),
                details: vec![detail],
                exit_code: None,
            };
        }
    };
    let exit_code = manifest.timing.exit_code;
    let expected = ExpectedProvenance {
        controller_sha: args.expected_controller_sha.clone(),
        subject_sha: args.expected_subject_sha.clone(),
        filter: expected.filter.to_owned(),
        count: expected.count,
        test_threads: expected.config.test_threads.clone(),
        mode: expected.mode_name.to_owned(),
        config: ManifestConfig::new(
            expected.config.nextest_profile.clone(),
            expected.config.nextest_config.clone(),
            expected.config.workflow_job_timeout_minutes,
        ),
        runner: expected.runner.clone(),
        policy: policy_snapshot(expected.config, expected.mode),
        execute_result: args.execute_result,
        sampler_healthy: true,
    };
    let mismatches = manifest.validate_provenance(&expected);
    if mismatches.is_empty() {
        return LaneProvenance {
            verdict: Ok(()),
            details: Vec::new(),
            exit_code,
        };
    }
    let details = mismatches.iter().map(ToString::to_string).collect();
    LaneProvenance {
        verdict: Err(NotClean::raised("stress provenance", mismatches.len())),
        details,
        exit_code,
    }
}

fn with_provenance(
    mut markdown: String,
    result: &Result<()>,
    details: &[String],
) -> Result<String> {
    if result.is_err() {
        invalidate_result(&mut markdown);
    }
    writeln!(markdown, "\n## Provenance")?;
    match result {
        Ok(()) => writeln!(
            markdown,
            "\nValidated against trusted workflow inputs: **yes**"
        )?,
        Err(error) => {
            writeln!(
                markdown,
                "\nValidated against trusted workflow inputs: **no**\n\n`{}`",
                markdown_cell(&format!("{error:#}"))
            )?;
            for detail in details.iter().take(100) {
                writeln!(markdown, "- `{}`", markdown_cell(detail))?;
            }
        }
    }
    Ok(markdown)
}

fn invalidate_result(markdown: &mut String) {
    for result in ["PASSED", "FAILED", "INCOMPLETE"] {
        let marker = format!("- Result: **{result}**");
        if let Some(index) = markdown.find(&marker) {
            markdown.replace_range(
                index..index + marker.len(),
                "- Result: **INVALID PROVENANCE**",
            );
            return;
        }
    }
    markdown.push_str("\n- Result: **INVALID PROVENANCE**\n");
}

/// The directory the whole campaign writes into, one level above its lanes.
///
/// Freshness is demanded here rather than per lane: the lanes are created
/// inside it as they run, so asking each of them for a directory that does not
/// exist yet would fail on the second one.
fn prepare_campaign_root(root: &Path) -> Result<()> {
    ensure!(
        !root
            .try_exists()
            .with_context(|| format!("inspect stress output {}", root.display()))?,
        "stress output already exists: {}; choose a fresh directory",
        root.display()
    );
    fs::create_dir_all(root).with_context(|| format!("create stress output {}", root.display()))?;
    Ok(())
}

fn prepare_raw_directory(paths: &Paths) -> Result<()> {
    ensure!(
        !paths
            .raw
            .try_exists()
            .with_context(|| format!("inspect stress output {}", paths.raw.display()))?,
        "stress output already exists: {}; choose a fresh directory",
        paths.raw.display()
    );
    fs::create_dir_all(&paths.raw)
        .with_context(|| format!("create stress output {}", paths.raw.display()))?;
    if let Some(envelopes) = &paths.envelopes {
        fs::create_dir_all(envelopes)
            .with_context(|| format!("create evidence directory {}", envelopes.display()))?;
    }
    fs::File::create(&paths.log)
        .with_context(|| format!("create stress command log {}", paths.log.display()))?;
    if let Some(lines) = &paths.lines {
        fs::File::create(lines)
            .with_context(|| format!("create line evidence sink {}", lines.display()))?;
    }
    Ok(())
}

/// Removes what the previous lane left at the subject's one `JUnit` path.
///
/// Staging is a copy with no proof of freshness, so the proof has to come from
/// the path being empty when the lane starts. Without this, a lane whose
/// nextest died before writing evidence would have its predecessor's `JUnit`
/// staged under its own name, and the report would attribute one lane's
/// failures to the other — the exact confusion a multi-lane campaign exists to
/// resolve.
fn clear_previous_lane_junit(junit: &Path) -> Result<()> {
    match fs::remove_file(junit) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("clear stress JUnit {}", junit.display())),
    }
}

fn stage_junit(source: &Path, destination: &Path) -> Result<()> {
    match fs::copy(source, destination) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(NotClean::reported("stress JUnit staging"))
        }
        Err(error) => Err(error).with_context(|| {
            format!(
                "stage stress JUnit {} as {}",
                source.display(),
                destination.display()
            )
        }),
    }
}

fn revision(root: &Path, expected: Option<&str>, label: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .with_context(|| format!("read {label} revision from {}", root.display()))?;
    if !output.status.success() {
        return Err(ChildFailure::captured(
            format!("read {label} revision"),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let actual = String::from_utf8(output.stdout)
        .context("git revision is not UTF-8")?
        .trim()
        .to_ascii_lowercase();
    ensure!(
        valid_sha(&actual),
        "{label} revision is not a full SHA: {actual:?}"
    );
    if let Some(expected) = expected {
        ensure!(
            actual.eq_ignore_ascii_case(expected),
            "{label} revision is {actual}, expected {expected}"
        );
    }
    Ok(actual)
}

fn choose_failure(
    first: Result<()>,
    second: Result<()>,
    third: Result<()>,
    fourth: Result<()>,
) -> Option<Error> {
    let mut verdict = None;
    let mut child = None;
    for result in [first, second, third, fourth] {
        let Err(error) = result else { continue };
        if error.downcast_ref::<ChildFailure>().is_some() {
            child.get_or_insert(error);
        } else if error.downcast_ref::<NotClean>().is_some() {
            verdict.get_or_insert(error);
        } else {
            return Some(error);
        }
    }
    child.or(verdict)
}

fn result_code(result: &Result<()>) -> i32 {
    result.as_ref().err().map_or(0, error_code)
}

fn error_code(error: &Error) -> i32 {
    if error.downcast_ref::<NotClean>().is_some() {
        1
    } else if let Some(failure) = error.downcast_ref::<ChildFailure>() {
        failure.exit_code()
    } else {
        1
    }
}

fn absolute_from(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn absolute_from_current(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .context("read current directory")?
            .join(path))
    }
}

fn ensure_report_outside_raw(raw: &Path, output: &Path) -> Result<()> {
    let raw = resolve_path_identity(raw)?;
    let output_identity = resolve_path_identity(output)?;
    ensure!(
        !output_identity.starts_with(&raw),
        "stress report output must be outside the raw evidence directory: {}",
        output.display()
    );
    let parent = output
        .parent()
        .context("stress report output has no parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create stress report parent {}", parent.display()))?;
    Ok(())
}

fn ensure_raw_outside_subject_evidence(raw: &Path, subject_junit: &Path) -> Result<()> {
    let raw = resolve_path_identity(raw)?;
    let evidence = subject_junit
        .parent()
        .context("subject JUnit path has no parent directory")?;
    let evidence = resolve_path_identity(evidence)?;
    ensure!(
        !raw.starts_with(&evidence) && !evidence.starts_with(&raw),
        "stress output must not overlap subject nextest evidence: {}",
        raw.display()
    );
    Ok(())
}

fn resolve_path_identity(path: &Path) -> Result<PathBuf> {
    let normalized = normalize_absolute(path)?;
    let mut existing = normalized.as_path();
    while !existing
        .try_exists()
        .with_context(|| format!("inspect path identity {}", existing.display()))?
    {
        existing = existing
            .parent()
            .with_context(|| format!("path has no existing ancestor: {}", path.display()))?;
    }
    let suffix = normalized
        .strip_prefix(existing)
        .context("derive unresolved path suffix")?;
    let resolved = fs::canonicalize(existing)
        .with_context(|| format!("resolve path identity {}", existing.display()))?;
    Ok(resolved.join(suffix))
}

fn normalize_absolute(path: &Path) -> Result<PathBuf> {
    ensure!(
        path.is_absolute(),
        "path is not absolute: {}",
        path.display()
    );
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                ensure!(
                    normalized.pop(),
                    "path escapes its filesystem root: {}",
                    path.display()
                );
            }
        }
    }
    Ok(normalized)
}

fn absolute_existing_directory(root: &Path, path: &Path, label: &str) -> Result<PathBuf> {
    let path = absolute_from(root, path);
    ensure!(
        path.is_dir(),
        "{label} directory does not exist: {}",
        path.display()
    );
    Ok(path)
}

fn absolute_existing_file(root: &Path, path: &Path, label: &str) -> Result<PathBuf> {
    let path = absolute_from(root, path);
    ensure!(path.is_file(), "{label} does not exist: {}", path.display());
    Ok(path)
}

fn valid_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_count(count: usize, max: usize) -> Result<()> {
    ensure!(count > 0, "stress count must be greater than zero");
    ensure!(count <= max, "stress count must not exceed {max}");
    Ok(())
}

fn markdown_cell(value: &str) -> String {
    value.replace(['\n', '\r', '`'], " ")
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    use super::*;

    fn command_lane_config() -> (StressConfig, StressModeConfig) {
        let config = StressConfig {
            lane: "workspace".to_owned(),
            backend: "wreq".to_owned(),
            ..StressConfig::default()
        };
        let mode = StressModeConfig {
            command: vec!["just".to_owned(), "test".to_owned(), "rtsan".to_owned()],
            ..StressModeConfig::default()
        };
        (config, mode)
    }

    /// The report has to expect the command the lane was told to run. Expecting
    /// the test runner instead condemns a lane that did exactly what the
    /// project asked, and a condemned lane leaves the comparison — which is how
    /// a campaign can run its sanitizer lanes and still report nothing about
    /// them.
    #[test]
    fn a_command_lane_is_expected_to_have_run_its_own_command() {
        let (config, mode) = command_lane_config();

        let expectation = ReportExpectation::new(
            &ProjectConfig::default(),
            &config,
            "rtsan",
            &mode,
            "all()",
            2,
        )
        .expect("a command lane needs no configured test runner");

        assert_eq!(expectation.runner.program, "just");
        assert_eq!(expectation.runner.prefix_args, ["test", "rtsan"]);
    }

    const VIOLATION: &str = "\
==2534==ERROR: RealtimeSanitizer: unsafe-library-call
Intercepted call to real-time unsafe function `malloc` in real-time context!
    #0 0x5628d3a1b2c0 in malloc (/opt/bin/suite_stress+0x1042c0)
    #1 0x5628d3c11f30 in kithara_audio::renderer::mix crates/kithara-audio/src/renderer/mix.rs:214:23
";

    fn command_lane(temp: &tempfile::TempDir, attempts: &str, log: &str) -> Paths {
        let paths = Paths::new(
            temp.path().to_path_buf(),
            &StressArtifactConfig {
                attempts: "attempts.json".to_owned(),
                log: "lane.log".to_owned(),
                ..StressArtifactConfig::default()
            },
        );
        fs::write(&paths.attempts, attempts).expect("write attempts fixture");
        fs::write(&paths.log, log).expect("write log fixture");
        paths
    }

    /// A sanitizer that fires on one attempt in two is green half the time. The
    /// campaign's job is to state that as a rate rather than as a verdict.
    #[test]
    fn a_command_lane_reports_how_many_attempts_the_command_rejected() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = command_lane(&temp, "[0,134,0,0]", "");

        let report = command_lane_report(&paths, 4, &StressEvidenceConfig::default());

        assert!(
            report.markdown.contains("Rejected attempts: `1`"),
            "{}",
            report.markdown
        );
        assert!(report.verdict.is_err());
    }

    /// An exit code says a lane is red. It does not say which contract broke,
    /// where, or on which attempts — and without that the reader is back to
    /// opening the log and guessing.
    #[test]
    fn a_command_lane_names_the_violation_the_sanitizer_reported() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = format!("{}0\n{VIOLATION}", stress_report::ATTEMPT_MARKER);
        let paths = command_lane(&temp, "[134,0]", &log);

        let report = command_lane_report(&paths, 2, &StressEvidenceConfig::default());

        assert!(
            report.markdown.contains("unsafe-library-call"),
            "{}",
            report.markdown
        );
        assert!(report.markdown.contains("malloc"), "{}", report.markdown);
        assert!(
            report
                .markdown
                .contains("crates/kithara-audio/src/renderer/mix.rs:214:23"),
            "{}",
            report.markdown
        );
        assert!(report.markdown.contains("50.00%"), "{}", report.markdown);
    }

    /// A rejected attempt the report cannot explain must say so. Printing only
    /// a count there reads as though the cause had been located.
    #[test]
    fn a_rejection_without_a_sanitizer_report_is_declared_unexplained() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = command_lane(&temp, "[1,0]", "error: could not compile `kithara-audio`\n");

        let report = command_lane_report(&paths, 2, &StressEvidenceConfig::default());

        assert!(
            report.markdown.contains("cannot name"),
            "{}",
            report.markdown
        );
    }

    #[test]
    fn a_command_lane_missing_its_attempts_does_not_read_as_a_clean_run() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(
            temp.path().to_path_buf(),
            &StressArtifactConfig {
                attempts: "absent.json".to_owned(),
                log: "lane.log".to_owned(),
                ..StressArtifactConfig::default()
            },
        );

        let report = command_lane_report(&paths, 2, &StressEvidenceConfig::default());

        assert!(
            report.markdown.contains("NO ATTEMPTS"),
            "{}",
            report.markdown
        );
        assert!(report.verdict.is_err());
    }

    #[test]
    fn a_failed_job_whose_lanes_all_passed_is_reported_as_unexplained() {
        let error = verify_campaign_result(ExecuteResult::Failure, &[Some(0), Some(0)])
            .expect_err("a red job with only clean lanes is not explained by its evidence");

        assert!(format!("{error:#}").contains("every lane finished cleanly"));
    }

    #[test]
    fn a_failed_job_is_explained_by_a_single_failing_lane() {
        verify_campaign_result(ExecuteResult::Failure, &[Some(0), Some(101)])
            .expect("one failing lane explains a failed job");
    }

    #[test]
    fn a_successful_job_says_nothing_about_lanes_beyond_their_own_manifests() {
        verify_campaign_result(ExecuteResult::Success, &[Some(0), Some(0)])
            .expect("a green job needs no campaign-level explanation");
    }

    #[test]
    fn clearing_the_previous_lane_junit_tolerates_a_path_that_is_already_empty() {
        let temp = tempfile::tempdir().expect("tempdir");
        let junit = temp.path().join("junit.xml");

        clear_previous_lane_junit(&junit).expect("an absent file is nothing to clear");
        fs::write(&junit, "stale").expect("write fixture");
        clear_previous_lane_junit(&junit).expect("a stale file is cleared");

        assert!(!junit.exists());
    }

    #[test]
    fn a_lane_that_is_not_a_plain_directory_name_is_refused() {
        for lane in ["../escape", "nested/lane", "", "/absolute"] {
            assert!(
                validate_lane_directory(lane).is_err(),
                "accepted `{lane}` as a lane"
            );
        }
        validate_lane_directory("reproduction-flash-off").expect("a plain name is a lane");
    }

    #[test]
    fn genuine_coordinator_error_has_failure_precedence() {
        let selected = choose_failure(
            Err(NotClean::reported("evidence")),
            Err(ChildFailure::inherited("nextest".to_owned(), Some(42))),
            Err(anyhow::anyhow!("cannot persist manifest")),
            Ok(()),
        )
        .expect("failure selected");

        assert_eq!(selected.to_string(), "cannot persist manifest");
    }

    #[test]
    fn child_status_has_precedence_over_evidence_verdict() {
        let selected = choose_failure(
            Err(NotClean::reported("evidence")),
            Err(ChildFailure::inherited("nextest".to_owned(), Some(42))),
            Ok(()),
            Ok(()),
        )
        .expect("failure selected");

        assert!(selected.downcast_ref::<ChildFailure>().is_some());
        assert_eq!(error_code(&selected), 42);
    }

    #[test]
    fn revision_contract_requires_a_full_hex_sha() {
        assert!(valid_sha("0123456789abcdef0123456789abcdef01234567"));
        assert!(!valid_sha("0123456789abcdef"));
        assert!(!valid_sha("g123456789abcdef0123456789abcdef01234567"));
    }

    #[test]
    fn provenance_failure_invalidates_a_passing_headline() {
        let mut markdown = "# Stress\n\n- Result: **PASSED**\n".to_owned();

        invalidate_result(&mut markdown);

        assert!(markdown.contains("Result: **INVALID PROVENANCE**"));
        assert!(!markdown.contains("Result: **PASSED**"));
    }

    #[test]
    fn report_output_cannot_overwrite_raw_evidence() {
        let temp = tempfile::tempdir().expect("tempdir");
        let raw = temp.path().join("raw");
        fs::create_dir(&raw).expect("create raw directory");

        let error = ensure_report_outside_raw(&raw, &raw.join("junit.xml"))
            .expect_err("raw overlap must be rejected");

        assert!(error.to_string().contains("outside the raw evidence"));
    }

    #[cfg(unix)]
    #[test]
    fn report_output_symlink_is_rejected_before_creating_inside_raw() {
        let temp = tempfile::tempdir().expect("tempdir");
        let raw = temp.path().join("raw");
        let alias = temp.path().join("alias");
        fs::create_dir(&raw).expect("create raw directory");
        symlink(&raw, &alias).expect("create raw alias");
        let output = alias.join("missing/report.md");

        let error = ensure_report_outside_raw(&raw, &output)
            .expect_err("symlinked raw overlap must be rejected");

        assert!(error.to_string().contains("outside the raw evidence"));
        assert!(!raw.join("missing").exists());
    }

    #[test]
    fn raw_output_cannot_overlap_subject_nextest_evidence() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("subject");
        fs::create_dir(&root).expect("create subject");
        let junit = root.join("target/nextest/stress/junit.xml");

        for raw in [
            root.join("target/nextest"),
            root.join("target/nextest/stress"),
            root.join("target/nextest/stress/raw"),
        ] {
            let error = ensure_raw_outside_subject_evidence(&raw, &junit)
                .expect_err("subject evidence overlap must be rejected");
            assert!(error.to_string().contains("must not overlap"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn raw_output_symlink_cannot_alias_subject_nextest_evidence() {
        let temp = tempfile::tempdir().expect("tempdir");
        let subject = temp.path().join("subject");
        let alias = temp.path().join("alias");
        fs::create_dir(&subject).expect("create subject");
        symlink(&subject, &alias).expect("create subject alias");
        let junit = subject.join("target/nextest/stress/junit.xml");
        let raw = alias.join("target/nextest/stress/raw");

        let error = ensure_raw_outside_subject_evidence(&raw, &junit)
            .expect_err("symlinked subject evidence overlap must be rejected");

        assert!(error.to_string().contains("must not overlap"));
    }
}
