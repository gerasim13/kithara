//! Portable repeated-test campaign and independent evidence verification.

use std::{
    fmt::Write as _,
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, ExitStatus},
};

use anyhow::{Context, Error, Result, ensure};
use clap::{ArgAction, Args, Subcommand};

use crate::{
    Ctx,
    stress_report::{self, StressReportArgs},
    stress_run::{self, StressRunSpec},
    verdict::{ChildFailure, NotClean},
};

mod environment;
mod manifest;
mod output;
pub(crate) mod pressure;
mod system;

use environment::{CampaignEnvironment, DiagnosticPolicy};
use manifest::{
    ExecuteResult, ExpectedProvenance, Features, Logging, Manifest, ManifestConfig, ManifestSpec,
    Mode, RunMetadata, Selection,
};
use pressure::Sampler;

struct Consts;

impl Consts {
    const DEFAULT_CONFIG: &str = ".config/nextest.toml";
    const DEFAULT_FILTER: &str = "all()";
    const DEFAULT_OUTPUT: &str = "target/stress";
    const DEFAULT_REPORT: &str = "target/stress-report.md";
    const DEFAULT_TEST_THREADS: &str = "num-cpus";
    const DEFAULT_JOB_TIMEOUT_MINUTES: u64 = 1_380;
}

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
    #[arg(long, default_value = Consts::DEFAULT_OUTPUT)]
    output: PathBuf,
    /// Controller-owned nextest configuration.
    #[arg(long, default_value = Consts::DEFAULT_CONFIG)]
    config_file: PathBuf,
    /// Nextest filterset selecting tests to repeat.
    #[arg(long, default_value = Consts::DEFAULT_FILTER)]
    filter: String,
    /// Number of times to run every selected test.
    #[arg(long, default_value_t = 50)]
    count: usize,
    /// Nextest concurrency (`num-cpus` or a positive integer).
    #[arg(long, default_value = Consts::DEFAULT_TEST_THREADS)]
    test_threads: String,
    /// Build and run the subject with Flash enabled.
    #[arg(long, default_value_t = true, action = ArgAction::Set)]
    flash: bool,
    /// Enable timing-perturbing diagnostic instrumentation.
    #[arg(long, default_value_t = false, action = ArgAction::Set)]
    diagnostics: bool,
    /// Enable no-block census in diagnostic mode.
    #[arg(long, default_value_t = false, action = ArgAction::Set)]
    no_block: bool,
    /// Include the Flash dump caller backtrace in diagnostic mode.
    #[arg(long, default_value_t = false, action = ArgAction::Set)]
    dump_thread_backtrace: bool,
    /// Trusted controller revision to compare with the checkout.
    #[arg(long)]
    expected_controller_sha: Option<String>,
    /// Trusted subject revision to compare with the checkout.
    #[arg(long)]
    expected_subject_sha: Option<String>,
    /// Outer job budget recorded in artifact provenance.
    #[arg(long, default_value_t = Consts::DEFAULT_JOB_TIMEOUT_MINUTES)]
    job_timeout_minutes: u64,
}

#[derive(Debug, Args)]
#[non_exhaustive]
pub struct ReportArgs {
    /// Downloaded raw evidence directory.
    #[arg(long)]
    raw: PathBuf,
    /// Markdown report destination.
    #[arg(long, default_value = Consts::DEFAULT_REPORT)]
    output: PathBuf,
    #[arg(long)]
    expected_controller_sha: String,
    #[arg(long)]
    expected_subject_sha: String,
    #[arg(long)]
    expected_filter: String,
    #[arg(long)]
    expected_count: usize,
    #[arg(long)]
    expected_test_threads: String,
    #[arg(long)]
    expected_mode: Mode,
    #[arg(long, action = ArgAction::Set)]
    expected_flash: bool,
    #[arg(long, action = ArgAction::Set)]
    expected_no_block: bool,
    #[arg(long, action = ArgAction::Set)]
    expected_dump_thread_backtrace: bool,
    #[arg(long, default_value_t = Consts::DEFAULT_JOB_TIMEOUT_MINUTES)]
    expected_job_timeout_minutes: u64,
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
        StressCommand::Report(args) => run_report(args),
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
    hang: PathBuf,
    inventory: PathBuf,
    junit: PathBuf,
    log: PathBuf,
    manifest: PathBuf,
    no_block: PathBuf,
    pressure: PathBuf,
    report: PathBuf,
}

impl Paths {
    fn new(raw: PathBuf) -> Self {
        Self {
            hang: raw.join("hang"),
            inventory: raw.join("inventory.json"),
            junit: raw.join("junit.xml"),
            log: raw.join("nextest.log"),
            manifest: raw.join("manifest.json"),
            no_block: raw.join("no-block.log"),
            pressure: raw.join("pressure.jsonl"),
            report: raw.join("stress-report.md"),
            raw,
        }
    }
}

fn run_campaign(args: &RunArgs, ctx: &Ctx) -> Result<()> {
    let subject_root = absolute_existing_directory(&ctx.root, &args.subject_root, "subject")?;
    let output = absolute_from(&ctx.root, &args.output);
    let paths = Paths::new(output);
    let config_file = absolute_existing_file(&ctx.root, &args.config_file, "nextest config")?;
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
    let subject_junit = subject_root.join("target/nextest/stress/junit.xml");
    let spec = StressRunSpec {
        inventory: paths.inventory.clone(),
        junit: subject_junit.clone(),
        config_file: config_file.clone(),
        filter: args.filter.clone(),
        count: args.count,
        test_threads: args.test_threads.clone(),
        flash: args.flash,
        no_block: args.diagnostics && args.no_block,
    };
    stress_run::validate(&spec)?;
    ensure_raw_outside_subject_evidence(&paths.raw, &subject_junit)?;
    let system = system::capture()?;
    let mode = Mode::from_diagnostics(args.diagnostics);
    let policy = DiagnosticPolicy {
        diagnostics: args.diagnostics,
        dump_thread_backtrace: args.dump_thread_backtrace,
        no_block: args.no_block,
    };
    let environment = CampaignEnvironment::new(&paths.raw, policy);
    let mut manifest = Manifest::start(
        manifest_spec(
            args,
            &paths,
            &config_file,
            controller_sha,
            subject_sha,
            mode,
            &environment,
        ),
        system.clone(),
    )?;
    prepare_raw_directory(&paths, args.diagnostics && args.no_block)?;
    let manifest_start = manifest.write_atomic(&paths.manifest);
    let sampler = Sampler::start(
        &paths.pressure,
        system.cgroup_v2.path.as_deref(),
        system.cgroup_v2.scope.as_str(),
    );

    let (primary, sampler_result) = match (manifest_start, sampler) {
        (Ok(()), Ok(sampler)) => {
            let primary =
                stress_run::run(&spec, &ctx.config, &subject_root, &paths.log, &|command| {
                    environment.apply(command)
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
    let report_result = render_raw_report(&paths, args.count, args.diagnostics && args.no_block);
    let final_error = choose_failure(
        primary,
        sampler_result.map(|_| ()),
        stage_result,
        report_result,
    );
    let final_code = final_error.as_ref().map_or(0, error_code);
    manifest.finalize(final_code, sampler_healthy)?;
    manifest.write_atomic(&paths.manifest)?;
    match final_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn manifest_spec(
    args: &RunArgs,
    paths: &Paths,
    config_file: &Path,
    controller_sha: String,
    subject_sha: String,
    mode: Mode,
    environment: &CampaignEnvironment,
) -> ManifestSpec {
    let no_block = args.diagnostics && args.no_block;
    let dump_thread_backtrace = args.diagnostics && args.dump_thread_backtrace;
    ManifestSpec {
        mode,
        config: ManifestConfig::stress(config_file.display().to_string(), args.job_timeout_minutes),
        controller_sha,
        subject_sha,
        run: github_run_metadata(),
        selection: Selection {
            filter: args.filter.clone(),
            count: args.count,
            test_threads: args.test_threads.clone(),
        },
        features: Features {
            flash: args.flash,
            diagnostics: args.diagnostics,
            no_block_requested: args.no_block,
            no_block,
            dump_thread_backtrace_requested: args.dump_thread_backtrace,
            dump_thread_backtrace,
        },
        logging: Logging {
            rust_backtrace: env_value(environment, "RUST_BACKTRACE"),
            rust_log: env_value(environment, "RUST_LOG"),
            flash_sync_trace: environment.value("KITHARA_FLASH_SYNC_TRACE").is_some(),
            flash_dump_thread_backtrace: environment.value("KITHARA_FLASH_SYNC_BT").is_some(),
            no_block_mode: environment
                .value("KITHARA_NO_BLOCK")
                .unwrap_or("off")
                .to_owned(),
            no_block_budget_ms: environment
                .value("KITHARA_NO_BLOCK_BUDGET_MS")
                .and_then(|value| value.parse().ok()),
            no_block_log: no_block.then(|| paths.no_block.display().to_string()),
            hang_dump_dir: paths.hang.display().to_string(),
            hang_prekill_secs: environment
                .value("KITHARA_HANG_PREKILL_SECS")
                .and_then(|value| value.parse().ok()),
            nextest_status_level: env_value(environment, "NEXTEST_STATUS_LEVEL"),
            nextest_final_status_level: env_value(environment, "NEXTEST_FINAL_STATUS_LEVEL"),
            nextest_show_progress: env_value(environment, "NEXTEST_SHOW_PROGRESS"),
        },
    }
}

fn github_run_metadata() -> RunMetadata {
    RunMetadata {
        repository: optional_env("GITHUB_REPOSITORY"),
        workflow: optional_env("GITHUB_WORKFLOW"),
        job: optional_env("GITHUB_JOB"),
        event: optional_env("GITHUB_EVENT_NAME"),
        run_id: optional_env("GITHUB_RUN_ID"),
        run_attempt: optional_env("GITHUB_RUN_ATTEMPT"),
    }
}

fn render_raw_report(paths: &Paths, count: usize, no_block: bool) -> Result<()> {
    let args = StressReportArgs::new(
        paths.junit.clone(),
        paths.inventory.clone(),
        paths.report.clone(),
        count,
    )
    .with_pressure(paths.pressure.clone())
    .with_optional_hang(paths.hang.is_dir().then(|| paths.hang.clone()))
    .with_optional_no_block(no_block.then(|| paths.no_block.clone()));
    stress_report::run(&args)
}

fn run_report(args: &ReportArgs) -> Result<()> {
    let raw = absolute_from_current(&args.raw)?;
    let paths = Paths::new(raw);
    let output = absolute_from_current(&args.output)?;
    ensure_report_outside_raw(&paths.raw, &output)?;
    let report_args = StressReportArgs::new(
        paths.junit.clone(),
        paths.inventory.clone(),
        output.clone(),
        args.expected_count,
    )
    .with_allow_missing(true)
    .with_pressure(paths.pressure.clone())
    .with_optional_hang(paths.hang.is_dir().then(|| paths.hang.clone()))
    .with_optional_no_block(
        (args.expected_mode == Mode::Diagnostic && args.expected_no_block)
            .then(|| paths.no_block.clone()),
    );
    let evidence = stress_report::run(&report_args);
    let (provenance, details) = verify_manifest(args, &paths.manifest);
    append_provenance(&output, &provenance, &details)?;
    choose_failure(evidence, provenance, Ok(()), Ok(())).map_or(Ok(()), Err)
}

fn verify_manifest(args: &ReportArgs, path: &Path) -> (Result<()>, Vec<String>) {
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
        filter: args.expected_filter.clone(),
        count: args.expected_count,
        test_threads: args.expected_test_threads.clone(),
        mode: args.expected_mode,
        flash: args.expected_flash,
        no_block_requested: args.expected_no_block,
        dump_thread_backtrace_requested: args.expected_dump_thread_backtrace,
        workflow_job_timeout_minutes: args.expected_job_timeout_minutes,
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

fn append_provenance(path: &Path, result: &Result<()>, details: &[String]) -> Result<()> {
    let mut markdown = fs::read_to_string(path)
        .unwrap_or_else(|_| "# Stress evidence\n\nStatus: **INVALID REPORT**\n".to_owned());
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
    stress_report::write_report(path, &markdown)
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

fn prepare_raw_directory(paths: &Paths, no_block: bool) -> Result<()> {
    ensure!(
        !paths
            .raw
            .try_exists()
            .with_context(|| format!("inspect stress output {}", paths.raw.display()))?,
        "stress output already exists: {}; choose a fresh directory",
        paths.raw.display()
    );
    fs::create_dir_all(&paths.hang)
        .with_context(|| format!("create stress output {}", paths.hang.display()))?;
    fs::File::create(&paths.log)
        .with_context(|| format!("create stress command log {}", paths.log.display()))?;
    if no_block {
        fs::File::create(&paths.no_block).with_context(|| {
            format!("create no-block evidence sink {}", paths.no_block.display())
        })?;
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

fn env_value(environment: &CampaignEnvironment, key: &str) -> String {
    environment.value(key).unwrap_or_default().to_owned()
}

fn optional_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

fn valid_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
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
