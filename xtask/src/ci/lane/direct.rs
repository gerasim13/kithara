use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::Args;
use kithara_devtools::{Ctx, lease};
use tracing::warn;

use super::declared;
use crate::{
    ci::{
        config::CiPins,
        environment::{CacheTrust, ci_in, expose_build_target, process_var},
        lane_build::{LaneBuild, SlotPool},
        process::Process,
        run::PipelineKind,
    },
    config::{CiLaneConfig, KitharaExt},
    consts,
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

/// What a lane is handed rather than works out: where this executor builds,
/// and the kind of pipeline it runs in.
///
/// A step spells its own build directory as `{target}`, and the checkout is
/// the wrong answer wherever the executor named another one - Cargo would
/// write where it was told while the lane looked for the binaries somewhere
/// nothing had written. The kind arrives only as this process's argument, so a
/// step that reads it - the weekly health report adds semver-checks - would
/// otherwise never see it. Nothing else is copied: a child already inherits
/// this process's environment, and [`Process`] layers what it is given on top.
///
/// A directory the lane claimed also has nightly Cargo mark each unit it
/// reuses, which is what the claim's pruning reads.
fn executor_vars(
    target_dir: Option<&Path>,
    kind: PipelineKind,
    claimed: bool,
) -> BTreeMap<OsString, OsString> {
    let mut vars = BTreeMap::from([(
        OsString::from("KITHARA_PIPELINE_KIND"),
        OsString::from(kind.name()),
    )]);
    if let Some(target) = target_dir {
        vars.insert(
            OsString::from("CARGO_TARGET_DIR"),
            target.as_os_str().to_owned(),
        );
    }
    if claimed {
        vars.insert(
            OsString::from(consts::MTIME_ON_USE_ENV),
            OsString::from("true"),
        );
    }
    vars
}

/// Where a lane builds, read from what the executor mounted. These are three
/// environments, not three attempts: each has exactly one answer.
#[derive(Debug)]
enum Target {
    /// A fleet runner: the first free slot of the lane's pool under its root.
    Slot(SlotPool),
    /// A fleet runner and a lane restoring a target snapshot: an empty
    /// directory of this run's own.
    Job(PathBuf),
    /// No fleet root: wherever Cargo was told to build, or the checkout.
    Named(Option<OsString>),
}

fn target(
    name: &str,
    lane: &CiLaneConfig,
    var: &dyn Fn(&str) -> Option<OsString>,
) -> Result<Target> {
    let Some(root) = var(consts::TARGET_ROOT_ENV).map(PathBuf::from) else {
        return Ok(Target::Named(var("CARGO_TARGET_DIR")));
    };
    if lane.target_snapshot.is_some() {
        let run = github_value("GITHUB_RUN_ID", var)?;
        let attempt = github_value("GITHUB_RUN_ATTEMPT", var)?;
        return Ok(Target::Job(
            root.join("jobs").join(format!("{run}-{attempt}-{name}")),
        ));
    }
    Ok(Target::Slot(SlotPool::fleet(
        &root,
        CacheTrust::read(var)?,
        name,
    )))
}

fn github_value(name: &str, var: &dyn Fn(&str) -> Option<OsString>) -> Result<String> {
    var(name)
        .and_then(|value| value.into_string().ok())
        .filter(|value| !value.is_empty())
        .with_context(|| format!("{name} must name the GitHub run a snapshot lane restores into"))
}

pub(crate) fn run(args: &LaneArgs, ctx: &Ctx) -> Result<()> {
    run_in(args, ctx, &process_var)
}

fn run_in(args: &LaneArgs, ctx: &Ctx, var: &dyn Fn(&str) -> Option<OsString>) -> Result<()> {
    let ext = KitharaExt::from_ctx(ctx)?;
    ext.ci.validate()?;
    let lane = lookup(&ext.ci.lanes, &args.lane)?;
    let pins = CiPins::load(&ctx.root.join(&ext.ci.pins))?;
    let (dir, cargo_dir, build) = match target(&args.lane, lane, var)? {
        Target::Slot(pool) => {
            let build = LaneBuild::claim(&ctx.root, &pool, ext.ci.lane_unit_window())?;
            let dir = build.dir().to_path_buf();
            let cargo_dir = hand_over(&ctx.root, &dir, var)?;
            (Some(dir), Some(cargo_dir), Some(build))
        }
        Target::Job(dir) => {
            fs::create_dir_all(&dir)
                .with_context(|| format!("creating the run's build directory {}", dir.display()))?;
            let cargo_dir = hand_over(&ctx.root, &dir, var)?;
            (Some(dir), Some(cargo_dir), None)
        }
        Target::Named(dir) => {
            let dir = dir.map(PathBuf::from);
            (dir.clone(), dir, None)
        }
    };
    let _lease = dir.as_deref().and_then(lease::hold);
    let process = Process::new(
        &ctx.root,
        executor_vars(cargo_dir.as_deref(), args.kind, build.is_some()),
    );
    let outcome = crate::ci::run::journalled(&process, &args.lane, || {
        declared::run(&process, lane, &pins, &ctx.config.tools, args.kind)
    });
    let settled = build.map_or(Ok(()), |build| build.settle(outcome.is_ok()));
    outcome.and(settled)
}

/// Where Cargo is told to build a directory of the fleet: always the
/// checkout's `target`, linked to `dir`, so the compiler cache sees one path
/// whichever directory the lane took. The job's later steps are told `dir`
/// itself.
fn hand_over(root: &Path, dir: &Path, var: &dyn Fn(&str) -> Option<OsString>) -> Result<PathBuf> {
    announce(dir, var);
    expose_build_target(root, dir, cfg!(windows), ci_in(var))
}

/// Tells the job's later steps where the lane built. GitHub reads `GITHUB_ENV`
/// into every step after this one; an executor without it has no later step to
/// tell. Only the upload of the build's timings reads it, so a lane that cannot
/// tell builds on and the upload finds nothing to send.
fn announce(dir: &Path, var: &dyn Fn(&str) -> Option<OsString>) {
    let Some(env_file) = var("GITHUB_ENV").map(PathBuf::from) else {
        return;
    };
    let written = OpenOptions::new()
        .append(true)
        .open(&env_file)
        .and_then(|mut file| writeln!(file, "{}={}", consts::LANE_TARGET_ENV, dir.display()));
    if let Err(error) = written {
        warn!(
            "later steps cannot find the lane build directory {}: writing {} failed: {error}",
            dir.display(),
            env_file.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{env, ffi::OsStr, fs, path::Path};

    use kithara_devtools::lock::FileLock;

    use super::*;
    use crate::ci::{
        config::{fixture, workspace_root},
        lane_build::lock_of,
    };

    /// A lane builds where the executor said. These runners are ephemeral and
    /// the checkout is deleted before the lane starts, so a build directory
    /// named from the checkout is empty on every job; the executor names one
    /// that outlives it, and a step's `{target}` has to mean that one or the
    /// lane looks for its binaries where nothing wrote any.
    #[test]
    fn a_lane_builds_where_the_executor_said() {
        let root = Path::new("/runner/_work/kithara/kithara");

        let handed = Process::new(
            root,
            executor_vars(
                Some(Path::new("/cache/target")),
                PipelineKind::Branch,
                false,
            ),
        );
        let bare = Process::new(root, executor_vars(None, PipelineKind::Branch, false));

        assert_eq!(handed.target_dir(), Path::new("/cache/target"));
        assert_eq!(bare.target_dir(), root.join("target"));
    }

    /// Pruning reads when a unit was last used, and only nightly Cargo told to
    /// mark reuse says so: a unit reused unmarked would look abandoned.
    #[test]
    fn a_claimed_directory_has_cargo_mark_what_it_reuses() {
        let claimed = executor_vars(
            Some(Path::new("/cache/lanes/review-lane-test-0")),
            PipelineKind::Branch,
            true,
        );
        let named = executor_vars(Some(Path::new("/work/target")), PipelineKind::Branch, false);

        assert_eq!(
            claimed.get(OsStr::new(consts::MTIME_ON_USE_ENV)),
            Some(&OsString::from("true"))
        );
        assert_eq!(
            named.get(OsStr::new(consts::MTIME_ON_USE_ENV)),
            None,
            "a directory nobody prunes has no use for the marks"
        );
    }

    /// The weekly health report runs semver-checks only when it reads the
    /// weekly kind, and a GitHub job hands the kind to this process alone.
    #[test]
    fn a_lane_tells_its_steps_the_kind_it_runs_in() {
        let process = Process::new(
            Path::new("/runner/_work/kithara/kithara"),
            executor_vars(None, PipelineKind::Weekly, false),
        );

        let command = process.command("just");

        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == "KITHARA_PIPELINE_KIND"
                    && value == Some(OsStr::new("weekly"))),
            "{command:?}"
        );
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

    /// The executor's variables as `ci lane` reads them.
    fn environment<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(*value))
        }
    }

    fn git_init(root: &Path) {
        let status = std::process::Command::new("git")
            .current_dir(root)
            .args(["init", "-q"])
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .status()
            .expect("run git init");
        assert!(status.success(), "git init");
    }

    /// A workspace at `root` declaring one lane whose only step succeeds.
    fn trivial_lane(root: &Path) -> (Ctx, LaneArgs) {
        if cfg!(windows) {
            lane_running(root, "cmd", r#"["/C", "exit", "0"]"#)
        } else {
            lane_running(root, "sh", r#"["-c", "exit 0"]"#)
        }
    }

    /// A workspace at `root` declaring one lane whose only step runs
    /// `program` with `step_args`, a TOML array.
    fn lane_running(root: &Path, program: &str, step_args: &str) -> (Ctx, LaneArgs) {
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

        let result = run_in(&args, &ctx, &environment(&[]));

        assert!(
            result.is_ok(),
            "a lane that needs no host profile must not fail resolving one: {result:?}"
        );
    }

    /// On the fleet a lane takes a slot of its own pool, records there what it
    /// built from, and tells the job's later steps where that slot is.
    #[test]
    fn a_lane_on_the_fleet_builds_in_a_slot_and_tells_the_job_where() {
        let temp = tempfile::tempdir().expect("create fixture workspace");
        let lanes = tempfile::tempdir().expect("create the fleet's build root");
        let (ctx, args) = trivial_lane(temp.path());
        git_init(temp.path());
        let github_env = temp.path().join("github-env");
        fs::write(&github_env, "").expect("create the job's GITHUB_ENV");
        let root = lanes.path().to_str().expect("a UTF-8 build root");
        let env_file = github_env.to_str().expect("a UTF-8 GITHUB_ENV path");

        run_in(
            &args,
            &ctx,
            &environment(&[
                (consts::TARGET_ROOT_ENV, root),
                ("KITHARA_CACHE_TRUST", "trusted"),
                ("GITHUB_ENV", env_file),
            ]),
        )
        .expect("lane runs");

        let slot = lanes.path().join("trusted-lane-trivial-0");
        assert!(
            slot.join(consts::SOURCES_FILE).exists(),
            "the slot must record what it was built from"
        );
        assert_eq!(
            fs::read_to_string(&github_env).expect("read GITHUB_ENV"),
            format!("{}={}\n", consts::LANE_TARGET_ENV, slot.display())
        );
    }

    /// The compiler cache keys a compilation on every `CARGO_*` variable, so a
    /// lane that handed Cargo its slot's own path would share nothing between
    /// slots: every slot would fill a compiler cache of its own.
    #[cfg(unix)]
    #[test]
    fn a_lane_hands_cargo_one_path_whichever_slot_it_builds_in() {
        let temp = tempfile::tempdir().expect("create fixture workspace");
        let lanes = tempfile::tempdir().expect("create the fleet's build root");
        let seen = temp.path().join("seen");
        let record = format!(
            r#"["-c", "printf '%s\\n%s' \"$CARGO_TARGET_DIR\" \"$(cd \"$CARGO_TARGET_DIR\" && pwd -P)\" > '{}'"]"#,
            seen.display()
        );
        let (ctx, args) = lane_running(temp.path(), "sh", &record);
        git_init(temp.path());
        let root = [(
            consts::TARGET_ROOT_ENV,
            lanes.path().to_str().expect("a UTF-8 build root"),
        )];
        let fleet = environment(&root);
        let slot = |index: usize| lanes.path().join(format!("review-lane-trivial-{index}"));
        let seen_by_the_step = || {
            let text = fs::read_to_string(&seen).expect("the step recorded its build path");
            let (cargo, resolved) = text
                .split_once('\n')
                .expect("the step recorded the path and what it resolves to");
            (PathBuf::from(cargo), PathBuf::from(resolved))
        };

        let lock = fs::File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_of(&slot(0)))
            .expect("open the first slot's lock");
        let held = FileLock::try_exclusive(lock).expect("another job holds the first slot");
        run_in(&args, &ctx, &fleet).expect("lane runs beside the held slot");
        let (beside, beside_slot) = seen_by_the_step();
        drop(held);
        run_in(&args, &ctx, &fleet).expect("lane runs in the released slot");
        let (released, released_slot) = seen_by_the_step();

        assert_eq!(beside, temp.path().join("target"));
        assert_eq!(
            released, beside,
            "Cargo must see one path whichever slot the lane took"
        );
        assert_eq!(beside_slot, fs::canonicalize(slot(1)).unwrap());
        assert_eq!(released_slot, fs::canonicalize(slot(0)).unwrap());
    }

    /// Outside a CI job the checkout's `target` is a developer's own build, so a
    /// lane that would link it to a slot refuses instead of deleting it.
    #[cfg(unix)]
    #[test]
    fn a_lane_outside_a_ci_job_keeps_the_checkouts_own_target() {
        let temp = tempfile::tempdir().expect("create fixture workspace");
        let lanes = tempfile::tempdir().expect("create the fleet's build root");
        let (ctx, args) = trivial_lane(temp.path());
        git_init(temp.path());
        let own = temp.path().join("target/debug/own-build");
        fs::create_dir_all(own.parent().expect("a build file has a directory"))
            .expect("create the developer's build");
        fs::write(&own, "built by hand").expect("write the developer's build");
        let root = lanes.path().to_str().expect("a UTF-8 build root");

        let result = run_in(
            &args,
            &ctx,
            &environment(&[(consts::TARGET_ROOT_ENV, root)]),
        );

        assert!(own.exists(), "the developer's build must survive the lane");
        let error = result.expect_err("a lane cannot link a target it must keep");
        assert!(
            format!("{error:#}").contains(&temp.path().join("target").display().to_string()),
            "the refusal must name the directory it kept: {error:#}"
        );
    }

    /// Only the upload of the build's timings reads where the lane built, so a
    /// job whose `GITHUB_ENV` cannot be written still runs its lane.
    #[test]
    fn a_lane_that_cannot_tell_the_job_where_it_built_still_runs() {
        let temp = tempfile::tempdir().expect("create fixture workspace");
        let lanes = tempfile::tempdir().expect("create the fleet's build root");
        let (ctx, args) = trivial_lane(temp.path());
        git_init(temp.path());
        let github_env = temp.path().join("missing/github-env");
        let root = lanes.path().to_str().expect("a UTF-8 build root");
        let env_file = github_env.to_str().expect("a UTF-8 GITHUB_ENV path");

        let result = run_in(
            &args,
            &ctx,
            &environment(&[(consts::TARGET_ROOT_ENV, root), ("GITHUB_ENV", env_file)]),
        );

        assert!(
            result.is_ok(),
            "a lane failed for the path only the timings upload reads: {result:?}"
        );
        assert!(
            lanes
                .path()
                .join("review-lane-trivial-0")
                .join(consts::SOURCES_FILE)
                .exists(),
            "the lane must still build in its slot"
        );
    }

    /// A snapshot restore needs an empty tree, so it never shares a slot.
    #[test]
    fn a_snapshot_lane_on_the_fleet_builds_in_a_directory_of_its_run() {
        let lane = CiLaneConfig {
            target_snapshot: Some("linux-test-release".to_owned()),
            ..CiLaneConfig::default()
        };

        let found = target(
            "deep-thing",
            &lane,
            &environment(&[
                (consts::TARGET_ROOT_ENV, "/cache/lanes"),
                ("GITHUB_RUN_ID", "7"),
                ("GITHUB_RUN_ATTEMPT", "2"),
            ]),
        )
        .unwrap();

        match found {
            Target::Job(dir) => assert_eq!(dir, Path::new("/cache/lanes/jobs/7-2-deep-thing")),
            other => panic!("a snapshot lane restores into a directory of its run, not {other:?}"),
        }
    }

    #[test]
    fn a_snapshot_lane_without_its_run_is_a_broken_environment() {
        let lane = CiLaneConfig {
            target_snapshot: Some("linux-test-release".to_owned()),
            ..CiLaneConfig::default()
        };

        let error = target(
            "deep-thing",
            &lane,
            &environment(&[(consts::TARGET_ROOT_ENV, "/cache/lanes")]),
        )
        .unwrap_err();

        assert!(
            error.to_string().contains("GITHUB_RUN_ID"),
            "the error must name what is missing: {error}"
        );
    }

    #[test]
    fn a_lane_off_the_fleet_builds_where_cargo_was_told() {
        let lane = CiLaneConfig::default();

        match target(
            "trivial",
            &lane,
            &environment(&[("CARGO_TARGET_DIR", "/work/target")]),
        )
        .unwrap()
        {
            Target::Named(Some(dir)) => assert_eq!(dir, OsString::from("/work/target")),
            other => panic!("a lane off the fleet builds where Cargo was told, not {other:?}"),
        }
        assert!(matches!(
            target("trivial", &lane, &environment(&[])).unwrap(),
            Target::Named(None)
        ));
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
