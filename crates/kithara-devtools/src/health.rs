use std::{
    fmt::Write,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use cargo_metadata::{Metadata, MetadataCommand};
use clap::Args;

use crate::{
    common::{project::ProjectConfig, timestamp::utc_timestamp},
    stages::{SharedStage, StageCommand},
};

struct Consts;
impl Consts {
    /// Substrings that mark an environment-level failure rather than a real
    /// regression — typically a missing tool or unpublished baseline.
    /// When any of these appear in the stage log on non-zero exit the stage
    /// is reported as SKIP instead of FAIL.
    const ENV_SKIP_MARKERS: &'static [&'static str] = &[
        "no such command:",
        "command not found",
        "not found in registry",
        "Library not loaded",
    ];
    const LOGS_DIR: &'static str = "target/health-logs";
    const REPORT_PATH: &'static str = "target/health-report.md";
    const STDOUT_TAIL_LINES: usize = 80;
}

#[derive(Debug, Args)]
pub struct HealthArgs {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Pass,
    Warn,
    Fail,
    Skip,
}

impl Status {
    const fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
            Self::Skip => "SKIP",
        }
    }
}

struct Stage {
    name: &'static str,
    program: &'static str,
    args: Vec<String>,
    advisory: bool,
    strict: bool,
}

impl Stage {
    fn new(name: &'static str, program: &'static str, args: &[&str]) -> Self {
        Self {
            name,
            program,
            args: args.iter().map(|s| (*s).to_string()).collect(),
            advisory: false,
            strict: false,
        }
    }

    const fn advisory(mut self) -> Self {
        self.advisory = true;
        self
    }

    /// A stage whose tool is not provisioned by this repository's own CI
    /// tooling (see `.config/ci-pins.toml` and `xtask/src/ci/image.rs`).
    /// `ENV_SKIP_MARKERS` exists for transient, environment-level noise on
    /// tools the fleet does provision; a stage marked strict never gets that
    /// pass, because a missing tool here is missing coverage the fleet
    /// never signed up to give, not a fluke worth hiding behind SKIP.
    const fn strict(mut self) -> Self {
        self.strict = true;
        self
    }

    fn packages(mut self, packages: &[String]) -> Self {
        for package in packages {
            self.args.push("--package".to_owned());
            self.args.push(package.clone());
        }
        self
    }

    /// Directories the tool should walk, for tools that take no exclude flag.
    fn paths(mut self, paths: &[String]) -> Self {
        self.args.extend(paths.iter().cloned());
        self
    }

    fn shared(name: &'static str, stage: SharedStage) -> Self {
        let StageCommand { program, args } = stage.health_command();
        Self {
            name,
            program,
            args,
            advisory: false,
            strict: false,
        }
    }
}

struct StageResult {
    name: &'static str,
    duration: Duration,
    note: Option<String>,
    status: Status,
    cmdline: String,
}

pub(crate) fn run(_args: &HealthArgs) -> Result<()> {
    let project = ProjectConfig::load(Path::new("."))?;
    let logs_dir = PathBuf::from(Consts::LOGS_DIR);
    fs::create_dir_all(&logs_dir).context("create health-logs directory")?;

    let stages = build_stages(&project)?;
    let total_start = Instant::now();
    let mut results = Vec::with_capacity(stages.len());

    for (idx, stage) in stages.iter().enumerate() {
        let result = run_stage(idx + 1, stage, &logs_dir);
        print_progress(idx + 1, stages.len(), &result);
        results.push(result);
    }

    let total = total_start.elapsed();
    write_report(&results, total, &logs_dir, &project.project.name)?;

    let failed = results.iter().filter(|r| r.status == Status::Fail).count();
    println!();
    println!(
        "health: {} stage(s) — {} failed in {}",
        results.len(),
        failed,
        format_duration(total),
    );
    println!("report: {}", Consts::REPORT_PATH);
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// What the stage list cannot state as a literal: project policy, and the
/// paths `cargo metadata` resolves it to.
#[derive(Default)]
struct Resolved {
    machete_paths: Vec<String>,
    semver_packages: Vec<String>,
    geiger_manifest: String,
    lockbud_toolchain: String,
}

fn build_stages(project: &ProjectConfig) -> Result<Vec<Stage>> {
    let metadata = MetadataCommand::new().no_deps().exec()?;
    let resolved = Resolved {
        machete_paths: walkable_packages(&metadata, &project.health.machete_exclude),
        semver_packages: project.health.semver_packages.clone(),
        geiger_manifest: manifest_of(&metadata, &project.health.geiger_package)?,
        lockbud_toolchain: format!("+{}", project.health.lockbud_toolchain),
    };
    Ok(build_stages_with(&resolved))
}

/// Workspace member directories relative to the workspace root, minus the
/// packages named. cargo-machete has no exclude flag: given no arguments it
/// walks the whole tree, and the only way to keep a package out of the walk is
/// to name every other one.
fn walkable_packages(metadata: &Metadata, excluded: &[String]) -> Vec<String> {
    let root = metadata.workspace_root.as_std_path();
    let mut paths = Vec::new();
    for package in metadata.workspace_packages() {
        if excluded.contains(&package.name.to_string()) {
            continue;
        }
        let Some(dir) = package.manifest_path.parent() else {
            continue;
        };
        let dir = dir.as_std_path();
        let relative = dir.strip_prefix(root).unwrap_or(dir);
        paths.push(relative.to_string_lossy().into_owned());
    }
    paths.sort();
    paths
}

/// One package's manifest, absolute. cargo-geiger rejects a relative
/// `--manifest-path` outright, so this cannot be a literal in the stage list.
fn manifest_of(metadata: &Metadata, package: &str) -> Result<String> {
    metadata
        .workspace_packages()
        .into_iter()
        .find(|candidate| candidate.name.as_str() == package)
        .map(|found| found.manifest_path.to_string())
        .with_context(|| format!("no workspace package named `{package}`"))
}

fn build_stages_with(resolved: &Resolved) -> Vec<Stage> {
    let Resolved {
        machete_paths,
        semver_packages,
        geiger_manifest,
        lockbud_toolchain,
    } = resolved;
    vec![
        Stage::shared("format-check", SharedStage::FmtCheck),
        Stage::new(
            "markdown-format-check",
            "cargo",
            &["xtask", "format", "--check", "--only", "markdown"],
        )
        .advisory(),
        Stage::shared("clippy", SharedStage::Clippy),
        Stage::shared("ast-grep-advisory", SharedStage::AstGrep).advisory(),
        Stage::shared("xtask-lint", SharedStage::Lint),
        Stage::new("quality-report", "cargo", &["xtask", "quality", "report"]),
        Stage::shared("typos", SharedStage::Typos),
        Stage::shared("similarity-strict", SharedStage::Similarity).advisory(),
        Stage::shared("orphans", SharedStage::Orphans),
        Stage::new("machete", "cargo", &["machete"]).paths(machete_paths),
        Stage::new("shear", "cargo", &["shear", "--deny-warnings"]),
        Stage::new("deny", "cargo", &["deny", "check"]),
        // The powerset owns which crates refuse a combination and holds them
        // out of the workspace pass, so this is one command rather than a shape
        // repeated here and in the deps recipe. `--no-dev-deps` is deliberately
        // absent: a crate whose tests need a feature its library does not
        // declare still has the wrong feature set, and the health run is where
        // that should show.
        Stage::new("hack-feature-powerset", "cargo", &["xtask", "powerset"]),
        // No workspace crate is published to crates.io (the workspace is an
        // application, not a crate release train), so the registry baseline
        // `check-release` looks up by default can never resolve. Comparing
        // against the tip of `main` instead makes this a real check: it
        // reports nothing on `main` itself (current == baseline) and flags
        // an actual public-API break on a branch that has drifted from it.
        // Which packages carry a contract worth comparing is project policy
        // and lives in `[health].semver_packages`; the workspace form is not
        // an option here, because cargo-semver-checks builds every package in
        // its own target directory and rebuilds that package's whole
        // dependency tree in it, twice.
        //
        // `--release-type minor` is what makes the comparison happen at all.
        // Left to derive the release type from the version numbers, the tool
        // reads the same version on both sides, assumes a major release — where
        // breaking is allowed — and skips every lint: 0 checks run and the
        // summary says no update is required, whatever the branch did to the
        // API. Stating minor asks the question worth asking: did this break?
        Stage::new(
            "semver-checks",
            "cargo",
            &[
                "semver-checks",
                "check-release",
                "--baseline-rev",
                "origin/main",
                "--release-type",
                "minor",
            ],
        )
        .packages(semver_packages),
        // cargo-geiger counts `unsafe` across a dependency tree, and a tree has
        // a root: pointed at the workspace it says only that the root manifest
        // is virtual. The census that means something here is the one rooted at
        // the facade, whose closure is what a consumer of this workspace links.
        Stage::new(
            "geiger",
            "cargo",
            &[
                "geiger",
                "--manifest-path",
                geiger_manifest,
                "--all-targets",
                "--all-dependencies",
                "--output-format",
                "Ascii",
            ],
        )
        .advisory(),
        // lockbud has no entry in `.config/ci-pins.toml` `[cargo_tools]` and no
        // install step in `xtask/src/ci/image.rs` / `docker/ci.Dockerfile`, so
        // the pinned CI image never carries it: `cargo lockbud` there is a
        // genuine "no such command". It is a rustc driver rather than a
        // crates.io package — it installs from git against one nightly, and it
        // only reads a workspace that same nightly compiled, which is why the
        // toolchain is selected here. `.strict()` keeps that missing coverage
        // from reading as a harmless SKIP; CONTEXT.md states what the image
        // still owes it.
        Stage::new(
            "lockbud-deadlock",
            "cargo",
            &[
                lockbud_toolchain,
                "lockbud",
                "-k",
                "deadlock",
                "--workspace",
            ],
        )
        .strict(),
        Stage::new("workspace-unused-pub", "cargo", &["workspace-unused-pub"]),
        Stage::new(
            "workspace-tests",
            "cargo",
            &["xtask", "test", "--lane=workspace"],
        ),
        Stage::new("doc-tests", "cargo", &["xtask", "test", "--lane=doc"]),
    ]
}

fn run_stage(idx: usize, stage: &Stage, logs_dir: &Path) -> StageResult {
    let cmdline = format!("{} {}", stage.program, stage.args.join(" "));
    let log_path = logs_dir.join(format!("{idx:02}-{}.log", stage.name));
    let log_file = match fs::File::create(&log_path) {
        Ok(f) => f,
        Err(e) => {
            return StageResult {
                cmdline,
                name: stage.name,
                status: Status::Fail,
                note: Some(format!("failed to open log: {e}")),
                duration: Duration::ZERO,
            };
        }
    };
    let stderr_file = match log_file.try_clone() {
        Ok(f) => f,
        Err(e) => {
            return StageResult {
                cmdline,
                name: stage.name,
                status: Status::Fail,
                note: Some(format!("failed to clone log handle: {e}")),
                duration: Duration::ZERO,
            };
        }
    };
    let start = Instant::now();
    let status_result = Command::new(stage.program)
        .args(&stage.args)
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(stderr_file))
        .status();
    let duration = start.elapsed();

    match status_result {
        Ok(s) if s.success() => StageResult {
            cmdline,
            duration,
            name: stage.name,
            status: Status::Pass,
            note: None,
        },
        Ok(s) => {
            let exit = s.code().unwrap_or(-1);
            let (status, note) = classify_failure(scan_env_skip_marker(&log_path), stage, exit);
            StageResult {
                cmdline,
                status,
                duration,
                name: stage.name,
                note: Some(note),
            }
        }
        Err(e) => {
            let kind = e.kind();
            let is_missing = matches!(kind, std::io::ErrorKind::NotFound);
            StageResult {
                cmdline,
                duration,
                name: stage.name,
                status: if is_missing {
                    Status::Skip
                } else {
                    Status::Fail
                },
                note: Some(format!("{kind}: {e}")),
            }
        }
    }
}

fn print_progress(idx: usize, total: usize, r: &StageResult) {
    println!(
        "[{idx:02}/{total:02}] {:<22} {:<5} {}{}",
        r.name,
        r.status.label(),
        format_duration(r.duration),
        r.note
            .as_ref()
            .map(|n| format!(" — {n}"))
            .unwrap_or_default(),
    );
}

fn write_report(
    results: &[StageResult],
    total: Duration,
    logs_dir: &Path,
    project_name: &str,
) -> Result<()> {
    let mut out = String::new();
    let timestamp = utc_timestamp();
    let total_str = format_duration(total);
    let failed = results.iter().filter(|r| r.status == Status::Fail).count();
    let overall = if failed == 0 { "PASS" } else { "FAIL" };

    let title = if project_name.is_empty() {
        "health report".to_owned()
    } else {
        format!("{project_name} health report")
    };
    let _ = write!(out, "# {title}\n\n");
    let _ = writeln!(out, "- generated_at_utc: {timestamp}");
    let _ = writeln!(out, "- total_duration: {total_str}");
    let _ = writeln!(
        out,
        "- overall: {overall} ({} stage(s), {failed} failed)",
        results.len()
    );
    let _ = write!(out, "- per-stage logs: `{}/`\n\n", logs_dir.display());
    out.push_str("Excluded by design (run separately): `mutants`, `coverage`, `dead`, ");
    out.push_str(
        "`test --lane=e2e`, `test --lane=selenium-firefox`, `wasm`, `bench`, `perf`, `memory-check`.\n\n",
    );

    out.push_str("## Summary\n\n");
    out.push_str("| # | Stage | Status | Duration | Notes |\n");
    out.push_str("|---|-------|--------|----------|-------|\n");
    for (idx, r) in results.iter().enumerate() {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} |",
            idx + 1,
            r.name,
            r.status.label(),
            format_duration(r.duration),
            r.note.clone().unwrap_or_default(),
        );
    }
    out.push('\n');

    out.push_str("## Stage details\n\n");
    for (idx, r) in results.iter().enumerate() {
        let log_path = logs_dir.join(format!("{:02}-{}.log", idx + 1, r.name));
        let _ = write!(
            out,
            "### {}. {} — {} ({})\n\n",
            idx + 1,
            r.name,
            r.status.label(),
            format_duration(r.duration),
        );
        let _ = write!(out, "```\n{}\n```\n\n", r.cmdline);
        if let Some(note) = &r.note {
            let _ = write!(out, "note: {note}\n\n");
        }
        let tail = read_log_tail(&log_path, Consts::STDOUT_TAIL_LINES);
        if !tail.is_empty() {
            let _ = write!(
                out,
                "<details><summary>last {} log lines (full: `{}`)</summary>\n\n```\n{}\n```\n\n</details>\n\n",
                Consts::STDOUT_TAIL_LINES,
                log_path.display(),
                tail,
            );
        }
    }

    fs::write(Consts::REPORT_PATH, out).context("write health report")?;
    Ok(())
}

/// Turn a non-zero exit into a status, given what (if anything) the log
/// matched as an environment-level marker. A strict stage never reads a
/// marker as SKIP: its tool has no pin, so a marker there means the tool is
/// genuinely absent, not flaking.
fn classify_failure(marker: Option<&'static str>, stage: &Stage, exit: i32) -> (Status, String) {
    match marker {
        Some(m) if stage.strict => (
            Status::Fail,
            format!("unprovisioned: {m} (exit {exit}) — not pinned/installed by CI tooling"),
        ),
        Some(m) => (Status::Skip, format!("environment: {m} (exit {exit})")),
        None if stage.advisory => (Status::Warn, format!("exit {exit}")),
        None => (Status::Fail, format!("exit {exit}")),
    }
}

fn scan_env_skip_marker(path: &Path) -> Option<&'static str> {
    let content = fs::read_to_string(path).ok()?;
    Consts::ENV_SKIP_MARKERS
        .iter()
        .copied()
        .find(|m| content.contains(m))
}

fn read_log_tail(path: &Path, n: usize) -> String {
    let Ok(content) = fs::read_to_string(path) else {
        return String::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}.{:01}s", d.subsec_millis() / 100)
    } else {
        format!("{}m{:02}s", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use clap::Subcommand;

    use super::*;
    use crate::CoreCommand;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn write_log(content: &str) -> PathBuf {
        let idx = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "kithara-xtask-health-{}-{idx}.log",
            std::process::id(),
        ));
        fs::write(&path, content).expect("write tmp log");
        path
    }

    #[test]
    fn health_xtask_stage_argv_is_parseable() {
        let stages = build_stages_with(&Resolved::default());
        let command = CoreCommand::augment_subcommands(clap::Command::new("xtask"));

        for stage in stages
            .iter()
            .filter(|stage| stage.args.first().is_some_and(|arg| arg == "xtask"))
        {
            let argv =
                std::iter::once("xtask").chain(stage.args.iter().skip(1).map(String::as_str));
            command
                .clone()
                .try_get_matches_from(argv)
                .unwrap_or_else(|error| panic!("invalid health stage '{}': {error}", stage.name));
        }

        let scopes = [
            crate::common::scope::Scope::default(),
            crate::common::scope::Scope::new(vec!["kithara-bufpool".to_owned()], vec![]),
            crate::common::scope::Scope::new(vec![], vec!["tests".into()]),
        ];
        for scope in scopes {
            for stage in SharedStage::AUDIT {
                let stage_command = stage.audit_command(&scope);
                if stage_command.program != "xtask"
                    || stage_command
                        .args
                        .last()
                        .is_some_and(|arg| arg == "__skip__")
                {
                    continue;
                }
                let argv =
                    std::iter::once("xtask").chain(stage_command.args.iter().map(String::as_str));
                command
                    .clone()
                    .try_get_matches_from(argv)
                    .unwrap_or_else(|error| {
                        panic!("invalid audit stage '{}': {error}", stage.audit_name())
                    });
            }
        }
    }

    #[test]
    fn skip_marker_no_such_command() {
        let log = write_log("error: no such command: `hack`\n");
        assert_eq!(scan_env_skip_marker(&log), Some("no such command:"));
        let _ = fs::remove_file(&log);
    }

    #[test]
    fn skip_marker_unpublished_baseline() {
        let log = write_log(
            "error: failed to retrieve index of crate versions from registry\n\
             Caused by:\n    kithara-abr not found in registry (crates.io).\n",
        );
        assert_eq!(scan_env_skip_marker(&log), Some("not found in registry"));
        let _ = fs::remove_file(&log);
    }

    #[test]
    fn skip_marker_lockbud_dylib_drift() {
        let log = write_log("dyld[691]: Library not loaded: @rpath/librustc_driver-XXX.dylib\n");
        assert_eq!(scan_env_skip_marker(&log), Some("Library not loaded"));
        let _ = fs::remove_file(&log);
    }

    #[test]
    fn skip_marker_genuine_failure_returns_none() {
        let log = write_log("test result: FAILED. 0 passed; 1 failed; 0 ignored\n");
        assert!(scan_env_skip_marker(&log).is_none());
        let _ = fs::remove_file(&log);
    }

    #[test]
    fn skip_marker_missing_log_returns_none() {
        let path = Path::new("/nonexistent/health-log.txt");
        assert!(scan_env_skip_marker(path).is_none());
    }

    #[test]
    fn classify_failure_marker_on_regular_stage_skips() {
        let stage = Stage::new("geiger", "cargo", &["geiger"]);
        let (status, note) = classify_failure(Some("no such command:"), &stage, 101);
        assert_eq!(status, Status::Skip);
        assert!(note.contains("environment:"));
    }

    #[test]
    fn classify_failure_marker_on_strict_stage_fails() {
        let stage = Stage::new("lockbud-deadlock", "cargo", &["lockbud"]).strict();
        let (status, note) = classify_failure(Some("no such command:"), &stage, 101);
        assert_eq!(status, Status::Fail);
        assert!(note.contains("unprovisioned:"));
    }

    #[test]
    fn classify_failure_no_marker_on_regular_stage_fails() {
        let stage = Stage::new("deny", "cargo", &["deny", "check"]);
        let (status, _) = classify_failure(None, &stage, 1);
        assert_eq!(status, Status::Fail);
    }

    #[test]
    fn classify_failure_no_marker_on_advisory_stage_warns() {
        let stage = Stage::new("geiger", "cargo", &["geiger"]).advisory();
        let (status, _) = classify_failure(None, &stage, 1);
        assert_eq!(status, Status::Warn);
    }

    #[test]
    fn lockbud_stage_is_strict() {
        let stages = build_stages_with(&Resolved::default());
        let lockbud = stages
            .iter()
            .find(|s| s.name == "lockbud-deadlock")
            .expect("lockbud-deadlock stage exists");
        assert!(
            lockbud.strict,
            "lockbud is not pinned by ci-pins.toml/image.rs, so its skip \
             markers must not read as harmless"
        );
    }

    #[test]
    fn machete_walks_the_packages_it_is_given() {
        let paths = ["crates/kithara-abr".to_owned(), "xtask".to_owned()];
        let stages = build_stages_with(&Resolved {
            machete_paths: paths.to_vec(),
            ..Resolved::default()
        });
        let machete = stages
            .iter()
            .find(|s| s.name == "machete")
            .expect("machete stage exists");
        assert_eq!(machete.args, ["machete", "crates/kithara-abr", "xtask"]);
    }

    fn this_workspace() -> Metadata {
        MetadataCommand::new()
            .no_deps()
            .exec()
            .expect("cargo metadata for this workspace")
    }

    #[test]
    fn a_package_left_out_is_not_walked() {
        let walked = walkable_packages(&this_workspace(), &["kithara-workspace-hack".to_owned()]);
        assert!(
            !walked.contains(&"crates/kithara-workspace-hack".to_owned()),
            "the excluded package is still in the walk: {walked:?}"
        );
    }

    #[test]
    fn every_other_package_is_still_walked() {
        let walked = walkable_packages(&this_workspace(), &["kithara-workspace-hack".to_owned()]);
        assert!(
            walked.contains(&"crates/kithara-abr".to_owned()),
            "naming one package dropped the rest: {walked:?}"
        );
    }

    #[test]
    fn the_geiger_root_resolves_to_an_absolute_manifest() {
        let manifest = manifest_of(&this_workspace(), "kithara").expect("the facade is a member");
        assert!(
            Path::new(&manifest).is_absolute(),
            "cargo-geiger rejects a relative --manifest-path: {manifest}"
        );
    }

    #[test]
    fn a_geiger_root_that_is_not_a_member_is_named() {
        let error = manifest_of(&this_workspace(), "kithara-not-a-package")
            .expect_err("a package that does not exist cannot resolve");
        assert!(
            error.to_string().contains("kithara-not-a-package"),
            "the error does not name the package: {error}"
        );
    }

    #[test]
    fn semver_checks_uses_git_baseline_not_registry() {
        let stages = build_stages_with(&Resolved::default());
        let semver = stages
            .iter()
            .find(|s| s.name == "semver-checks")
            .expect("semver-checks stage exists");
        assert!(
            semver.args.iter().any(|a| a == "--baseline-rev"),
            "workspace crates are never published to crates.io, so the default \
             registry baseline can never resolve"
        );
    }

    #[test]
    fn semver_checks_names_the_packages_it_compares() {
        let stages = build_stages_with(&Resolved {
            semver_packages: vec!["kithara".to_owned()],
            ..Resolved::default()
        });
        let semver = stages
            .iter()
            .find(|s| s.name == "semver-checks")
            .expect("semver-checks stage exists");
        assert!(
            semver
                .args
                .windows(2)
                .any(|w| w == ["--package", "kithara"]),
            "configured package is missing from the command: {:?}",
            semver.args
        );
    }

    #[test]
    fn semver_checks_states_the_release_type() {
        let stages = build_stages_with(&Resolved {
            semver_packages: vec!["kithara".to_owned()],
            ..Resolved::default()
        });
        let semver = stages
            .iter()
            .find(|s| s.name == "semver-checks")
            .expect("semver-checks stage exists");
        assert!(
            semver
                .args
                .windows(2)
                .any(|w| w == ["--release-type", "minor"]),
            "branch and baseline carry the same version, so a derived release \
             type is major and every lint skips: {:?}",
            semver.args
        );
    }

    #[test]
    fn semver_checks_never_takes_the_whole_workspace() {
        let stages = build_stages_with(&Resolved {
            semver_packages: vec!["kithara".to_owned()],
            ..Resolved::default()
        });
        let semver = stages
            .iter()
            .find(|s| s.name == "semver-checks")
            .expect("semver-checks stage exists");
        assert!(
            !semver.args.iter().any(|a| a == "--workspace"),
            "one target directory and one full dependency build per package \
             puts the workspace form beyond any nightly budget"
        );
    }
}
