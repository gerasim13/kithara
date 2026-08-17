use std::process::Command;

use anyhow::Result;
use clap::Args;

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
    let status = Command::new("cargo")
        .args(["semver-checks", "check-release", "--workspace"])
        .args(["--exclude", Consts::EXCLUDE])
        .args(["--baseline-rev", &args.baseline])
        .status()?;
    if !status.success() {
        return Err(NotClean::reported("semver-checks"));
    }
    Ok(())
}
