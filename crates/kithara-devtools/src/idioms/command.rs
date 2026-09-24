use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};
use cargo_metadata::MetadataCommand;
use clap::Args;
use rayon::prelude::*;

use super::{
    checks,
    checks::{Check, Context, registry},
    config::IdiomsConfig,
};
use crate::common::{
    baseline::{Baseline, RatchetDiff},
    exclude::apply_lint_excludes,
    project::ProjectConfig,
    report,
    scan::Scan,
    scope::Scope,
    violation::{Report, Violation},
};

#[derive(Debug, Default, Args)]
pub struct IdiomsArgs {
    #[arg(long)]
    pub report: Option<PathBuf>,
    #[arg(long, default_value = ".config/idioms")]
    pub config_dir: PathBuf,
    #[arg(long = "check")]
    pub check: Vec<String>,
    /// Restrict scan to specific crate(s) by name. Repeatable.
    #[arg(long = "crate", value_name = "NAME")]
    pub crates: Vec<String>,
    /// Restrict scan to workspace-relative path(s). Repeatable.
    #[arg(long = "path", value_name = "PATH")]
    pub paths: Vec<PathBuf>,
    /// Skip the dirty-tree gate that protects `--fix` from uncommitted edits.
    #[arg(long = "allow-dirty")]
    pub allow_dirty: bool,
    /// Apply safe idiom collapses in place, then re-run detection.
    #[arg(long)]
    pub fix: bool,
    #[arg(long)]
    pub json: bool,
    /// Print each check's wall time to stderr, slowest first.
    #[arg(long)]
    pub timings: bool,
    #[arg(long = "update-baseline")]
    pub update_baseline: bool,
}

pub(crate) fn run(args: &IdiomsArgs) -> Result<()> {
    validate(args)?;

    let metadata = MetadataCommand::new().exec()?;
    let workspace_root = metadata.workspace_root.as_std_path().to_path_buf();
    let config = IdiomsConfig::load(&args.config_dir)?;
    let scope = Scope::new(args.crates.clone(), args.paths.clone());

    let fix_scan = Scan::new(&workspace_root);
    let ctx = Context {
        workspace_root: &workspace_root,
        metadata: &metadata,
        config: &config,
        scope: &scope,
        scan: &fix_scan,
    };

    let registry = registry();
    let known_ids: HashSet<&str> = registry.iter().map(|c| c.id()).collect();

    let filter: Option<HashSet<&str>> = if args.check.is_empty() {
        None
    } else {
        for requested in &args.check {
            if !known_ids.contains(requested.as_str()) {
                bail!("unknown idioms check id: '{requested}'");
            }
        }
        Some(args.check.iter().map(String::as_str).collect())
    };

    if args.fix {
        run_fix(&registry, &filter, &ctx, args.allow_dirty)?;
    }

    let project = ProjectConfig::load(&workspace_root)?;

    let scan = Scan::new(&workspace_root);
    let ctx = Context { scan: &scan, ..ctx };

    let selected: Vec<&dyn Check> = registry
        .iter()
        .map(Box::as_ref)
        .filter(|check| filter.as_ref().is_none_or(|ids| ids.contains(check.id())))
        .collect();
    let ran: Vec<&'static str> = selected.iter().map(|check| check.id()).collect();

    let outcomes = run_checks(&selected, &ctx, &scope, &project, &workspace_root)?;

    if args.timings {
        let rows: Vec<_> = ran
            .iter()
            .copied()
            .zip(outcomes.iter().map(|(elapsed, _)| *elapsed))
            .collect();
        report::print_timings("idioms", &rows);
    }

    let mut report = Report::default();
    for (_, violations) in outcomes {
        report.extend(violations);
    }

    if args.update_baseline {
        let new_baseline = Baseline::from_report(&report);
        let path = new_baseline.save(&args.config_dir)?;
        let total: usize = new_baseline.checks.values().map(BTreeMap::len).sum();
        println!(
            "wrote idioms baseline ({} entry across {} check(s)) to {}",
            total,
            new_baseline.checks.len(),
            path.display(),
        );
        return Ok(());
    }

    let baseline = Baseline::load(&args.config_dir)?;
    let baseline = if scope.is_empty() {
        baseline
    } else {
        baseline.filter_keys(|k| scope.key_in_scope(k))
    };
    let diff = baseline.diff(&report.violations);

    if let Some(path) = &args.report {
        let md = report::render_markdown(&report, &ran, &diff);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create report dir: {}", parent.display()))?;
        }
        fs::write(path, md).with_context(|| format!("write report: {}", path.display()))?;
        eprintln!("wrote markdown report to {}", path.display());
    } else if args.json {
        print!("{}", report::render_json(&report, &ran, &diff));
    } else {
        print_report(&report, &ran, &diff);
    }

    if diff.has_failures() {
        bail!(
            "idioms ratchet failed: {} regression(s), {} new violation(s)",
            diff.regressions.len(),
            diff.new_violations.len(),
        );
    }
    Ok(())
}

fn apply_common_exclusions(
    report: &mut Report,
    policy: checks::CheckPolicy,
    path_patterns: &[String],
    module_patterns: &[String],
    workspace_root: &Path,
) {
    if policy.keeps_source_findings() {
        return;
    }
    apply_lint_excludes(report, path_patterns, module_patterns, workspace_root);
}

/// Runs each selected check, returning its wall time and its violations in
/// registry order.
///
/// A parsed `syn::File` holds `proc_macro2` spans and is neither `Send` nor
/// `Sync`, so a tree can be neither shared between checks nor moved across a
/// thread; what spreads is the checks themselves. The shared scan means they
/// no longer each re-walk the tree or re-read the same bytes.
///
/// Memory scales with the pool, not the registry: a worker holds one file's
/// tree at a time. Measured with twelve workers the run peaks at 2.0 GB; a
/// host with many more cores than memory sets `RAYON_NUM_THREADS`.
fn run_checks(
    selected: &[&dyn Check],
    ctx: &Context<'_>,
    scope: &Scope,
    project: &ProjectConfig,
    workspace_root: &Path,
) -> Result<Vec<(Duration, Vec<Violation>)>> {
    selected
        .par_iter()
        .map(|check| {
            let effective_scope = check.policy().scope(scope);
            let check_ctx = Context {
                scope: &effective_scope,
                ..*ctx
            };
            let started = Instant::now();
            let mut check_report = Report::default();
            check_report.extend(check.run(&check_ctx)?);
            apply_common_exclusions(
                &mut check_report,
                check.policy(),
                &project.lint_exclude.runtime_paths(),
                &project.lint_exclude.modules,
                workspace_root,
            );
            Ok((started.elapsed(), check_report.violations))
        })
        .collect()
}

fn run_fix(
    registry: &[Box<dyn Check>],
    filter: &Option<HashSet<&str>>,
    ctx: &Context<'_>,
    allow_dirty: bool,
) -> Result<()> {
    crate::util::ensure_clean_tree(allow_dirty, "xtask lint idioms")?;
    let mut writes = 0;
    let mut changes = Vec::new();
    let mut skipped = Vec::new();
    for check in registry {
        if let Some(filter) = filter
            && !filter.contains(check.id())
        {
            continue;
        }
        let effective_scope = check.policy().scope(ctx.scope);
        let check_ctx = Context {
            scope: &effective_scope,
            ..*ctx
        };
        let outcome = check.fix(&check_ctx)?;
        writes += outcome.writes;
        changes.extend(
            outcome
                .changes
                .into_iter()
                .map(|value| format!("idioms/{}: {value}", check.id())),
        );
        skipped.extend(
            outcome
                .skipped
                .into_iter()
                .map(|value| format!("idioms/{}: {value}", check.id())),
        );
    }
    println!("idioms fix: wrote {writes} file(s)");
    for change in changes {
        println!("  changed — {change}");
    }
    for reason in skipped {
        println!("  skipped — {reason}");
    }
    Ok(())
}

fn print_report(report: &Report, ran: &[&'static str], diff: &RatchetDiff<'_>) {
    if ran.is_empty() {
        println!("OK: no idioms checks registered yet.");
        return;
    }
    if report.violations.is_empty() && diff.improvements.is_empty() {
        println!(
            "OK: {} idioms check(s) passed: {}.",
            ran.len(),
            ran.join(", ")
        );
        return;
    }

    report::print_grouped(report, diff);
    println!(
        "summary: {deny} deny, {warn} warn, {regr} regression(s), {new} new across {n} check(s).",
        deny = report.deny_count(),
        warn = report.warn_count(),
        regr = diff.regressions.len(),
        new = diff.new_violations.len(),
        n = ran.len(),
    );
}

fn validate(args: &IdiomsArgs) -> Result<()> {
    if args.update_baseline && (args.report.is_some() || args.json) {
        bail!("--update-baseline cannot be combined with --report or --json");
    }
    if args.json && args.report.is_some() {
        bail!("--json and --report are mutually exclusive");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{scope::Scope, violation::Violation};

    #[test]
    fn derivable_policy_adds_tests_and_xtask_only_to_empty_scope() {
        let root = Path::new("/workspace");
        assert_eq!(Scope::default().roots(root), vec![root.join("crates")]);
        assert_eq!(
            Scope::default().with_workspace_sources().roots(root),
            vec![root.join("crates"), root.join("tests"), root.join("xtask")]
        );
        let explicit = Scope::new(vec!["kithara-queue".into()], vec![]);
        assert_eq!(
            explicit.clone().with_workspace_sources().roots(root),
            explicit.roots(root)
        );
    }

    #[test]
    fn ordinary_checks_drop_cfg_test_findings_but_derivable_checks_keep_them() {
        let dir = tempfile::tempdir().unwrap();
        let source = "#[cfg(test)] mod tests { fn only_test() {} }";
        fs::write(dir.path().join("fixture.rs"), source).unwrap();
        let mut ordinary = Report::default();
        ordinary.extend([Violation::deny("ordinary", "fixture.rs:1:0", "test")]);
        apply_common_exclusions(
            &mut ordinary,
            checks::CheckPolicy::Default,
            &[],
            &[],
            dir.path(),
        );
        assert!(ordinary.violations.is_empty());

        let mut derivable = Report::default();
        derivable.extend([Violation::deny(
            "derivable_display",
            "fixture.rs:1:0",
            "test",
        )]);
        apply_common_exclusions(
            &mut derivable,
            checks::CheckPolicy::WorkspaceSources,
            &[],
            &[],
            dir.path(),
        );
        assert_eq!(derivable.violations.len(), 1);
    }

    #[test]
    fn event_policy_stays_production_only() {
        use checks::{Check, derivable_event::DerivableEvent};
        let event = DerivableEvent;
        assert_eq!(event.policy(), checks::CheckPolicy::Default);
    }
}
