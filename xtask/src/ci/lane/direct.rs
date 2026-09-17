use std::{collections::BTreeMap, env, ffi::OsString, path::Path};

use anyhow::{Result, bail};
use clap::Args;
use kithara_devtools::Ctx;

use super::declared;
use crate::{
    ci::{config::CiPins, lane_build::LaneBuild, process::Process, run::PipelineKind},
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
    lane: String,
    // Required, not defaulted: the only caller is a generated workflow that
    // already passes this on every invocation, so a default would only paper
    // over a resolution the caller owns.
    #[arg(long, value_enum)]
    kind: PipelineKind,
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

/// The one thing a lane is handed rather than works out: where this executor
/// builds.
///
/// A step spells its own build directory as `{target}`, and the checkout is
/// the wrong answer wherever the executor named another one - Cargo would
/// write where it was told while the lane looked for the binaries somewhere
/// nothing had written. Nothing else is copied: a child already inherits this
/// process's environment, and [`Process`] layers what it is given on top.
fn executor_vars(target_dir: Option<OsString>) -> BTreeMap<OsString, OsString> {
    target_dir
        .map(|target| BTreeMap::from([(OsString::from("CARGO_TARGET_DIR"), target)]))
        .unwrap_or_default()
}

pub(crate) fn run(args: &LaneArgs, ctx: &Ctx) -> Result<()> {
    run_in(args, ctx, env::var_os("CARGO_TARGET_DIR"))
}

fn run_in(args: &LaneArgs, ctx: &Ctx, target_dir: Option<OsString>) -> Result<()> {
    let ext = KitharaExt::from_ctx(ctx)?;
    ext.ci.validate()?;
    let lane = lookup(&ext.ci.lanes, &args.lane)?;
    let pins = CiPins::load(&ctx.root.join(&ext.ci.pins))?;
    // Every lane but a snapshot restore builds in the directory named after
    // it, which checkouts of other content share; a snapshot lane is handed a
    // private one.
    let build = match (&target_dir, &lane.target_snapshot) {
        (Some(dir), None) => Some(LaneBuild::claim(&ctx.root, Path::new(dir))?),
        _ => None,
    };
    let process = Process::new(&ctx.root, executor_vars(target_dir));
    let outcome = declared::run(&process, lane, &pins, &ctx.config.tools, args.kind);
    let settled = build.map_or(Ok(()), |build| build.settle(outcome.is_ok()));
    outcome.and(settled)
}

#[cfg(test)]
mod tests {
    use std::{env, fs, path::Path};

    use super::*;
    use crate::ci::config::{fixture, workspace_root};

    /// A lane builds where the executor said. These runners are ephemeral and
    /// the checkout is deleted before the lane starts, so a build directory
    /// named from the checkout is empty on every job; the executor names one
    /// that outlives it, and a step's `{target}` has to mean that one or the
    /// lane looks for its binaries where nothing wrote any.
    #[test]
    fn a_lane_builds_where_the_executor_said() {
        let root = Path::new("/runner/_work/kithara/kithara");

        let handed = Process::new(root, executor_vars(Some(OsString::from("/cache/target"))));
        let bare = Process::new(root, executor_vars(None));

        assert_eq!(handed.target_dir(), Path::new("/cache/target"));
        assert_eq!(bare.target_dir(), root.join("target"));
    }

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
        let lanes = BTreeMap::from([
            (
                "linux-lint".to_owned(),
                CiLaneConfig {
                    label: "linux-lint".to_owned(),
                    ..CiLaneConfig::default()
                },
            ),
            (
                "apple-test".to_owned(),
                CiLaneConfig {
                    label: "apple-test".to_owned(),
                    ..CiLaneConfig::default()
                },
            ),
        ]);
        let found = lookup(&lanes, "apple-test").expect("a declared lane is found");
        assert_eq!(
            found.label, "apple-test",
            "lookup must return the lane that was asked for, not merely any lane"
        );
    }

    /// A workspace at `root` declaring one lane whose only step succeeds.
    fn trivial_lane(root: &Path) -> (Ctx, LaneArgs) {
        let (program, step_args) = if cfg!(windows) {
            ("cmd", r#"["/C", "exit", "0"]"#)
        } else {
            ("sh", r#"["-c", "exit 0"]"#)
        };

        let root = root.to_path_buf();
        fixture()
            .pins
            .write(&root.join("ci-pins.toml"))
            .expect("write fixture pins into the temporary workspace");

        // The fixture runs here, so it names here: a lane refuses a machine
        // that is not the one it declared, and that refusal is the subject of
        // other tests, not of this one.
        let os = env::consts::OS;
        let config_text = format!(
            r#"
[ext.ci]
pins = "ci-pins.toml"

[ext.ci.lanes.trivial]
cache_group = "host"
label = "fixture"
os = "{os}"
program = "{program}"
role = "gate"
timeout_minutes = 1

[[ext.ci.lanes.trivial.steps]]
label = "run"
args = {step_args}
"#
        );
        let ctx = Ctx::new(
            root,
            toml::from_str(&config_text).expect("parse fixture lane config"),
        );
        let args = LaneArgs {
            lane: "trivial".to_owned(),
            kind: PipelineKind::Branch,
        };
        (ctx, args)
    }

    /// `ci run` requires `KITHARA_CI_HOST_CONFIG` and bails without it
    /// (`xtask/src/ci/run.rs`). So a lane that reaches its own work at all,
    /// with the ambient environment left untouched, is already the proof
    /// that `ci lane` resolved no host profile: the fixture lane's one step
    /// is `sh -c "exit 0"` (`cmd /C exit 0` on Windows), and `Ok` means
    /// execution got there.
    #[test]
    fn a_lane_reaches_its_own_work_with_no_host_profile_resolved() {
        let temp = tempfile::tempdir().expect("create fixture workspace");
        let (ctx, args) = trivial_lane(temp.path());

        let result = run_in(&args, &ctx, None);

        assert!(
            result.is_ok(),
            "a lane that needs no host profile must not fail resolving one: {result:?}"
        );
    }

    /// A lane building in its shared directory records the content it built
    /// from, which is what lets the next checkout of other content rebuild
    /// instead of reusing these artifacts.
    #[test]
    fn a_lane_claims_the_shared_directory_it_builds_in() {
        let temp = tempfile::tempdir().expect("create fixture workspace");
        let target = tempfile::tempdir().expect("create lane build directory");
        let (ctx, args) = trivial_lane(temp.path());
        let status = std::process::Command::new("git")
            .current_dir(temp.path())
            .args(["init", "-q"])
            .status()
            .expect("run git init");
        assert!(status.success(), "git init");

        run_in(&args, &ctx, Some(target.path().as_os_str().to_owned())).expect("lane runs");

        assert!(
            target.path().join(".kithara-lane-sources").exists(),
            "the shared lane directory must record what it was built from"
        );
    }

    /// The test above cannot fail loudly enough alone: a resolution bug that
    /// only sometimes needs a host profile could still return `Ok`. This
    /// pins the negative direction against the source directly: a GitHub
    /// container has no host profile installed, so this entrypoint must
    /// never name the machinery that would resolve one.
    ///
    /// Only the production half of this file is scanned - up to the
    /// `#[cfg(test)]` boundary - because the forbidden names below are
    /// themselves text inside this test module, and a census that read its
    /// own assertion would always trip on its own data.
    #[test]
    fn ci_lane_never_names_host_profile_machinery() {
        let source = fs::read_to_string(workspace_root().join("xtask/src/ci/lane/direct.rs"))
            .expect("direct.rs is readable");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("direct.rs has a production half before its test module");
        // A census that scanned nothing would pass every assertion below. The
        // split is a text match, so an earlier `#[cfg(test)]` would truncate
        // the production half silently; this is what makes that loud.
        assert!(
            production.contains("fn run(args: &LaneArgs"),
            "the production half must still hold the entrypoint being censused"
        );
        for forbidden in ["CiConfig::load", "CiEnvironment", "KITHARA_CI_HOST_CONFIG"] {
            assert!(
                !production.contains(forbidden),
                "a GitHub container has no host profile, so `ci lane` must never resolve \
                 one; found `{forbidden}` in direct.rs's production code"
            );
        }
    }

    #[test]
    fn declared_lanes_own_target_snapshot_lifecycle_for_both_executors() {
        let root = workspace_root();
        let direct = fs::read_to_string(root.join("xtask/src/ci/lane/direct.rs"))
            .expect("direct lane source is readable");
        let declared = fs::read_to_string(root.join("xtask/src/ci/lane/declared.rs"))
            .expect("declared lane source is readable");
        let production = |source: String| {
            source
                .split("#[cfg(test)]")
                .next()
                .expect("lane source has a production half")
                .to_owned()
        };

        assert!(!production(direct).contains("snapshot::"));
        let declared = production(declared);
        assert!(declared.contains("snapshot::restore_for_lane"));
        assert!(declared.contains("snapshot::publish_for_lane"));
        assert!(declared.contains("could not publish optional target snapshot"));
    }
}
