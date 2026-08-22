use std::{collections::BTreeSet, process::Command};

use anyhow::{Context, Result, bail};
use cargo_metadata::MetadataCommand;
use clap::Args;
use serde::Deserialize;

use crate::{Ctx, util::check_tool, verdict::NotClean};

struct Consts;
impl Consts {
    const INSTALL_HINT: &'static str = "cargo install cargo-semver-checks";
    /// Fuzz targets are not a published surface and carry no baseline.
    const EXCLUDE: &'static str = "kithara-fuzz";
}

#[derive(Debug, Args)]
pub struct SemverArgs {
    /// Revision to compare against. The workspace is unpublished, so the
    /// baseline is a revision here rather than a crates.io release.
    #[arg(long, default_value = "HEAD~1")]
    pub baseline: String,
}

/// Runs the workspace's public-surface comparison against a revision.
///
/// The wrapper exists for the lease, not for the flags. `cargo semver-checks`
/// builds rustdoc for every crate twice and clones the baseline revision into
/// `CARGO_TARGET_DIR`, which on Linux runners is a volume the host budgets. A
/// bare `cargo` invocation holds no job lease, and between two crates it holds
/// no `.cargo-lock` either, so a reclaim in a sibling job reads the whole
/// target as abandoned: the clone this run had been reading for seven minutes
/// disappeared, and the failure surfaced as `failed to canonicalize manifest
/// path` on whichever crate came next. Running under `xtask` puts the lane
/// behind the same lease every other build lane holds.
pub(crate) fn run(args: &SemverArgs, _ctx: &Ctx) -> Result<()> {
    check_tool(
        "cargo",
        &["semver-checks", "--version"],
        Consts::INSTALL_HINT,
    )?;
    let added = packages_added_since(&args.baseline)?;
    for name in &added {
        println!(
            "semver-checks: {name} has no counterpart in {}, nothing to compare",
            args.baseline
        );
    }
    let status = Command::new("cargo")
        .args(["semver-checks", "check-release", "--workspace"])
        .args(["--exclude", Consts::EXCLUDE])
        .args(added.iter().flat_map(|name| ["--exclude", name.as_str()]))
        .args(["--baseline-rev", &args.baseline])
        .status()?;
    if !status.success() {
        return Err(NotClean::reported("semver-checks"));
    }
    Ok(())
}

/// Workspace members that do not exist at `baseline`.
///
/// `--workspace` makes `cargo-semver-checks` resolve every member by name
/// inside the baseline checkout, and a name it cannot find fails the whole run.
/// A crate introduced by this revision has no earlier surface to break, so
/// leaving it out is the answer the comparison would give, not a hole in it.
fn packages_added_since(baseline: &str) -> Result<Vec<String>> {
    let baseline_members = members_at(baseline)?;
    let metadata = MetadataCommand::new().no_deps().exec()?;
    let mut added: Vec<String> = metadata
        .workspace_packages()
        .into_iter()
        .map(|package| package.name.to_string())
        .filter(|name| !baseline_members.contains(name))
        .collect();
    added.sort();
    Ok(added)
}

/// Workspace member names recorded in `baseline`'s lockfile.
fn members_at(baseline: &str) -> Result<BTreeSet<String>> {
    let path = format!("{baseline}:Cargo.lock");
    let output = Command::new("git").args(["show", &path]).output()?;
    if !output.status.success() {
        bail!(
            "git show {path} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let lock = String::from_utf8(output.stdout).context("baseline Cargo.lock is not UTF-8")?;
    local_packages(&lock)
}

/// The path-sourced entries of a lockfile.
///
/// Registry and git packages carry a `source` there; path packages do not,
/// which is what separates the workspace's own crates from its dependencies.
fn local_packages(lock: &str) -> Result<BTreeSet<String>> {
    let lockfile: Lockfile = toml::from_str(lock).context("parse baseline Cargo.lock")?;
    Ok(lockfile
        .package
        .into_iter()
        .filter(|package| package.source.is_none())
        .map(|package| package.name)
        .collect())
}

#[derive(Debug, Deserialize)]
struct Lockfile {
    #[serde(default)]
    package: Vec<LockPackage>,
}

#[derive(Debug, Deserialize)]
struct LockPackage {
    name: String,
    source: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::local_packages;

    const LOCK: &str = r#"
version = 4

[[package]]
name = "anyhow"
version = "1.0.100"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "kithara-decode"
version = "0.0.1-alpha4"
dependencies = ["anyhow"]

[[package]]
name = "firewheel-web-audio"
version = "0.7.0"
source = "git+https://github.com/example/firewheel#0000000"
"#;

    #[test]
    fn local_packages_keeps_path_entries() -> anyhow::Result<()> {
        let names = local_packages(LOCK)?;

        assert!(names.contains("kithara-decode"));
        Ok(())
    }

    #[test]
    fn local_packages_drops_registry_entries() -> anyhow::Result<()> {
        let names = local_packages(LOCK)?;

        assert!(!names.contains("anyhow"));
        Ok(())
    }

    #[test]
    fn local_packages_drops_git_entries() -> anyhow::Result<()> {
        let names = local_packages(LOCK)?;

        assert!(!names.contains("firewheel-web-audio"));
        Ok(())
    }
}
