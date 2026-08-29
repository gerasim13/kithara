use std::{collections::BTreeMap, ffi::OsString};

use anyhow::{Result, bail};
use clap::Args;
use kithara_devtools::Ctx;

use super::declared;
use crate::{
    ci::{config::CiPins, process::Process, run::PipelineKind},
    config::{CiLaneConfig, KitharaExt},
};

/// Run one declared lane in the environment the executor already prepared.
///
/// `ci run` is the other way into the same lane body: it prepares the cache
/// roots, the compiler cache and the build-cache lease first, because the
/// GitLab executor arrives with none of them. A GitHub job's container is
/// started with exactly those variables already set, so preparing them again
/// would be a second owner of the same state.
#[derive(Debug, Args)]
pub(crate) struct LaneArgs {
    /// The declared lane to run.
    pub(crate) lane: String,
    #[arg(long, value_enum, default_value_t = PipelineKind::Branch)]
    pub(crate) kind: PipelineKind,
}

fn lookup<'a>(lanes: &'a BTreeMap<String, CiLaneConfig>, name: &str) -> Result<&'a CiLaneConfig> {
    match lanes.get(name) {
        Some(lane) => Ok(lane),
        None => bail!(
            "`{name}` is not a declared CI lane; this repository has {}",
            lanes.keys().cloned().collect::<Vec<_>>().join(", ")
        ),
    }
}

pub(crate) fn run(args: &LaneArgs, ctx: &Ctx) -> Result<()> {
    let ext = KitharaExt::from_ctx(ctx)?;
    ext.ci.validate()?;
    let lane = lookup(&ext.ci.lanes, &args.lane)?;
    let pins = CiPins::load(&ctx.root.join(&ext.ci.pins))?;
    let vars: BTreeMap<OsString, OsString> = BTreeMap::new();
    let process = Process::new(&ctx.root, vars);
    declared::run(&process, lane, &pins, args.kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A lane name that is not in the catalog must answer with the catalog,
    // not with whatever the machine happens to be missing.
    #[test]
    fn an_unknown_lane_answers_with_the_lanes_this_repository_has() {
        let lanes = BTreeMap::from([("linux-lint".to_owned(), CiLaneConfig::default())]);
        let error = lookup(&lanes, "linux-lnt").expect_err("a misspelled lane is refused");
        assert!(
            error.to_string().contains("linux-lint"),
            "the error must list the lanes: {error}"
        );
    }

    #[test]
    fn a_known_lane_is_returned() {
        let lanes = BTreeMap::from([("linux-lint".to_owned(), CiLaneConfig::default())]);
        assert!(lookup(&lanes, "linux-lint").is_ok());
    }
}
