//! Portable repeated-test campaign and independent evidence verification.

use std::{
    collections::BTreeSet,
    fmt::Write as _,
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, ExitStatus},
};

use anyhow::{Context, Error, Result, ensure};
use clap::{Args, Subcommand};

use crate::{
    Ctx,
    common::project::{StressArtifactConfig, StressConfig, StressModeConfig},
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

impl Paths {
    fn new(raw: PathBuf, artifacts: &StressArtifactConfig) -> Self {
        Self {
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
    let runner = configured_lane(&ctx.config, &config.lane, &config.backend, &mode.features)?;
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
    stress_run::validate(&spec)?;
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
            let primary = stress_run::run(&spec, &subject_root, &paths.log, &|command| {
                environment.apply(command);
            });
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
    let stage_result = stage_junit(&subject_junit, &paths.junit);
    let report_result = render_raw_report(&paths, count, config);
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
    let mut failure = None;
    for lane_name in &lanes {
        let mode = config.mode(lane_name)?;
        let runner = configured_lane(&ctx.config, &config.lane, &config.backend, &mode.features)?;
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
        let lane = stress_report::lane_report(&report_args)?;
        let expectation = ReportExpectation {
            config,
            mode_name: lane_name,
            mode,
            filter: &filter,
            count,
            runner,
        };
        let (provenance, details) = verify_manifest(args, &expectation, &paths.manifest);
        let trusted = provenance.is_ok();
        let body = with_provenance(lane.markdown, &provenance, &details)?;
        writeln!(sections, "\n# Lane `{}`\n", markdown_cell(lane_name))?;
        sections.push_str(&body);
        // Only a lane that verified against its expected identity may stand in
        // a comparison. Numbers from one that did not are of unknown origin,
        // and putting them beside trustworthy ones is how a campaign reports a
        // difference between lanes that is really a difference between runs.
        if trusted {
            measured.push((lane_name.clone(), lane.rates));
        }
        let lane_failure = choose_failure(lane.verdict, provenance, Ok(()), Ok(()));
        if let Some(error) = lane_failure
            && failure.is_none()
        {
            failure = Some(error);
        }
    }

    let mut document = stress_report::render_lane_comparison(&measured, lanes.len());
    document.push_str(&sections);
    stress_report::write_report(&output, &document)?;
    failure.map_or(Ok(()), Err)
}

fn verify_manifest(
    args: &ReportArgs,
    expected: &ReportExpectation<'_>,
    path: &Path,
) -> (Result<()>, Vec<String>) {
    let manifest = match Manifest::read(path) {
        Ok(manifest) => manifest,
        Err(error) => {
            let detail = format!("{error:#}");
            return (Err(error), vec![detail]);
        }
    };
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
        return (Ok(()), Vec::new());
    }
    let details = mismatches.iter().map(ToString::to_string).collect();
    (
        Err(NotClean::raised("stress provenance", mismatches.len())),
        details,
    )
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
