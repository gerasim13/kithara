use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};
use clap::Args;
use kithara_devtools::Ctx;
use tracing::warn;

use super::declared;
use crate::{
    ci::{
        build_dir::{LaneTarget, Target},
        config::CiPins,
        environment::process_var,
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
/// Cargo writes where `CARGO_TARGET_DIR` names, and [`Process::target_dir`]
/// reads the same value, so the lane looks for its binaries where they were
/// written; the checkout is the wrong answer wherever the executor named
/// another directory. The kind arrives only as this process's argument, so a
/// step that reads it - the weekly health report adds semver-checks - would
/// otherwise never see it. Nothing else is copied: a child already inherits
/// this process's environment, and [`Process`] layers what it is given on top.
fn executor_vars(target_dir: Option<&Path>, kind: PipelineKind) -> BTreeMap<OsString, OsString> {
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
    vars
}

pub(crate) fn run(args: &LaneArgs, ctx: &Ctx) -> Result<()> {
    run_in(args, ctx, &process_var)
}

fn run_in(args: &LaneArgs, ctx: &Ctx, var: &dyn Fn(&str) -> Option<OsString>) -> Result<()> {
    let ext = KitharaExt::from_ctx(ctx)?;
    ext.ci.validate()?;
    let lane = lookup(&ext.ci.lanes, &args.lane)?;
    let pins = CiPins::load(&ctx.root.join(&ext.ci.pins))?;
    let target = Target::enter(
        &ctx.root,
        LaneTarget {
            name: &args.lane,
            window: ext.ci.lane_unit_window(),
        },
        var,
    )?;
    if let Target::Alias { build, .. } = &target {
        announce(build.path(), var);
    }
    let process = Process::new(&ctx.root, executor_vars(target.cargo_dir(), args.kind));
    crate::ci::run::journalled(&process, &args.lane, || {
        let result = declared::run(&process, lane, &pins, &ctx.config.tools, args.kind);
        if var("RUSTC_WRAPPER").is_some_and(|wrapper| !wrapper.is_empty()) {
            let on_github =
                crate::job::github_in(&|name| var(name).and_then(|value| value.into_string().ok()));
            crate::ci::run::note_compiler_cache(
                &process,
                ctx.config.tools.program("sccache"),
                on_github,
            );
        }
        result
    })
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

    use kithara_devtools::lease;

    use super::*;
    use crate::ci::config::{fixture, workspace_root};

    /// A lane builds where the executor said. These runners are ephemeral and
    /// the checkout is deleted before the lane starts, so a build directory
    /// named from the checkout is empty on every job; the executor names one
    /// that outlives it, and the lane has to look for its binaries in that
    /// one.
    #[test]
    fn a_lane_builds_where_the_executor_said() {
        let root = Path::new("/runner/_work/kithara/kithara");

        let handed = Process::new(
            root,
            executor_vars(Some(Path::new("/cache/target")), PipelineKind::Branch),
        );
        let bare = Process::new(root, executor_vars(None, PipelineKind::Branch));

        assert_eq!(handed.target_dir(), Path::new("/cache/target"));
        assert_eq!(bare.target_dir(), root.join("target"));
    }

    /// The weekly health report runs semver-checks only when it reads the
    /// weekly kind, and a GitHub job hands the kind to this process alone.
    #[test]
    fn a_lane_tells_its_steps_the_kind_it_runs_in() {
        let process = Process::new(
            Path::new("/runner/_work/kithara/kithara"),
            executor_vars(None, PipelineKind::Weekly),
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

    /// A workspace at `root` declaring one lane of the given freshness whose
    /// only step succeeds.
    fn trivial_lane(root: &Path, freshness: &str) -> (Ctx, LaneArgs) {
        if cfg!(windows) {
            lane_running(root, "cmd", r#"args = ["/C", "exit", "0"]"#, freshness)
        } else {
            lane_running(root, "sh", r#"args = ["-c", "exit 0"]"#, freshness)
        }
    }

    /// A workspace at `root` declaring one lane of the given freshness whose
    /// only step, labelled `run`, runs `program` as the TOML lines `step` say.
    fn lane_running(root: &Path, program: &str, step: &str, freshness: &str) -> (Ctx, LaneArgs) {
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
freshness = "{freshness}"
label = "fixture"
os = "{os}"
program = "{program}"
role = "gate"
timeout_minutes = 1

[[ext.ci.lanes.trivial.steps]]
label = "run"
{step}
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
        let (ctx, args) = trivial_lane(temp.path(), "mtime");

        let result = run_in(&args, &ctx, &environment(&[]));

        assert!(
            result.is_ok(),
            "a lane that needs no host profile must not fail resolving one: {result:?}"
        );
    }

    /// In a CI job a lane builds in a directory of its own beside the alias
    /// the executor named, while Cargo is told the alias: every lane compiles
    /// at the one path, so the compiler cache's keys match across lanes and
    /// runners. The job's later steps and its artifact paths reach the same
    /// directory.
    #[cfg(unix)]
    #[test]
    fn a_lane_in_a_ci_job_builds_in_its_own_directory_behind_the_alias() {
        let temp = tempfile::tempdir().expect("create fixture workspace");
        let builds = tempfile::tempdir().expect("create the runner's build root");
        let seen = temp.path().join("seen");
        let record = format!(
            r#"args = ["-c", "printf '%s' \"$CARGO_TARGET_DIR\" > '{}'"]"#,
            seen.display()
        );
        let (ctx, args) = lane_running(temp.path(), "sh", &record, "mtime");
        let github_env = temp.path().join("github-env");
        fs::write(&github_env, "").expect("create the job's GITHUB_ENV");
        let alias = builds.path().join(consts::BUILD_ALIAS);
        let own = builds.path().join("trivial");

        run_in(
            &args,
            &ctx,
            &environment(&[
                ("CI", "true"),
                ("CARGO_TARGET_DIR", alias.to_str().expect("a UTF-8 alias")),
                (
                    "GITHUB_ENV",
                    github_env.to_str().expect("a UTF-8 GITHUB_ENV"),
                ),
            ]),
        )
        .expect("lane runs");

        assert_eq!(
            fs::read_to_string(&seen).expect("the step recorded its build path"),
            alias.display().to_string(),
            "Cargo is told the alias every lane shares"
        );
        assert_eq!(fs::read_link(&alias).unwrap(), Path::new("trivial"));
        assert!(
            own.join(lease::FILE).is_file(),
            "the lane leased its own build"
        );
        assert_eq!(
            fs::read_to_string(&github_env).expect("read GITHUB_ENV"),
            format!("{}={}\n", consts::LANE_TARGET_ENV, own.display())
        );
        assert_eq!(
            fs::canonicalize(temp.path().join("target")).unwrap(),
            fs::canonicalize(&own).unwrap(),
            "artifact paths under the checkout's target reach the lane's build"
        );
    }

    /// The rebuild check asks cargo what the next job of this commit would
    /// build, and fails the lane on any unit named; a suite the next job
    /// reuses whole passes. It needs nothing but the build the step left.
    #[cfg(unix)]
    #[test]
    fn a_rebuild_check_fails_a_suite_the_next_job_would_build_again() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("create fixture workspace");
        let status = temp.path().join("status");
        let just = temp.path().join("just");
        fs::write(
            &just,
            format!(
                "#!/bin/sh\ncase \"$*\" in *--no-run*) cat '{}' >&2 ;; esac\n",
                status.display()
            ),
        )
        .expect("write the just double");
        fs::set_permissions(&just, fs::Permissions::from_mode(0o755))
            .expect("make the just double runnable");
        let (mut ctx, args) = lane_running(
            temp.path(),
            "just",
            "args = [\"test\", \"run\", \"--timings\"]\nrebuild_check = true",
            "mtime",
        );
        ctx.config.tools = toml::from_str(&format!("[just]\nprogram = \"{}\"\n", just.display()))
            .expect("parse the tools table");
        let anywhere = environment(&[]);

        fs::write(&status, "   Compiling probe v0.0.0 (/w)\n").expect("write cargo's answer");
        let error = run_in(&args, &ctx, &anywhere)
            .expect_err("a unit the next job would build fails the lane");
        assert!(
            format!("{error:#}").contains("Compiling probe v0.0.0"),
            "{error:#}"
        );

        fs::write(&status, "       Fresh probe v0.0.0 (/w)\n").expect("write cargo's answer");
        run_in(&args, &ctx, &anywhere).expect("a suite the next job reuses whole passes");
    }

    /// A job compiling through the cache says what the cache carried for it;
    /// one that compiles without it has no cache to ask.
    #[cfg(unix)]
    #[test]
    fn a_lane_that_compiled_through_the_cache_reads_its_counts() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("create fixture workspace");
        let asked = temp.path().join("asked");
        let sccache = temp.path().join("sccache");
        fs::write(
            &sccache,
            format!(
                "#!/bin/sh\necho \"$@\" >> '{}'\nprintf 'Cache hits 3\\nCache misses 1\\nCache write errors 0\\n'\n",
                asked.display()
            ),
        )
        .expect("write the cache double");
        fs::set_permissions(&sccache, fs::Permissions::from_mode(0o755))
            .expect("make the cache double runnable");
        let (mut ctx, args) = trivial_lane(temp.path(), "mtime");
        ctx.config.tools =
            toml::from_str(&format!("[sccache]\nprogram = \"{}\"\n", sccache.display()))
                .expect("parse the tools table");

        run_in(&args, &ctx, &environment(&[])).expect("lane runs without a wrapper");
        assert!(
            !asked.exists(),
            "a lane with no wrapper has no cache to ask"
        );

        run_in(&args, &ctx, &environment(&[("RUSTC_WRAPPER", "sccache")]))
            .expect("lane runs through the wrapper");
        assert_eq!(
            fs::read_to_string(&asked).expect("the cache was asked"),
            "--show-stats\n"
        );
    }

    /// Outside a CI job a directory named like an alias is still just where
    /// Cargo was told to build: nothing is linked, and the checkout's own
    /// `target`, a developer's build, is left alone.
    #[cfg(unix)]
    #[test]
    fn a_lane_outside_a_ci_job_enters_no_build_directory() {
        let temp = tempfile::tempdir().expect("create fixture workspace");
        let builds = tempfile::tempdir().expect("create a build root");
        let (ctx, args) = trivial_lane(temp.path(), "mtime");
        let own = temp.path().join("target/debug/own-build");
        fs::create_dir_all(own.parent().expect("a build file has a directory"))
            .expect("create the developer's build");
        fs::write(&own, "built by hand").expect("write the developer's build");
        let named = builds.path().join(consts::BUILD_ALIAS);

        run_in(
            &args,
            &ctx,
            &environment(&[("CARGO_TARGET_DIR", named.to_str().expect("a UTF-8 path"))]),
        )
        .expect("lane runs");

        assert!(own.exists(), "the developer's build must survive the lane");
        assert!(
            !builds.path().join("trivial").exists(),
            "a lane outside a CI job entered a build directory"
        );
    }

    /// Only the upload of the build's timings reads where the lane built, so a
    /// job whose `GITHUB_ENV` cannot be written still runs its lane.
    #[test]
    fn a_lane_that_cannot_tell_the_job_where_it_built_still_runs() {
        let temp = tempfile::tempdir().expect("create fixture workspace");
        let builds = tempfile::tempdir().expect("create the runner's build root");
        let (ctx, args) = trivial_lane(temp.path(), "mtime");
        let github_env = temp.path().join("missing/github-env");
        let alias = builds.path().join(consts::BUILD_ALIAS);

        let result = run_in(
            &args,
            &ctx,
            &environment(&[
                ("CI", "true"),
                ("CARGO_TARGET_DIR", alias.to_str().expect("a UTF-8 alias")),
                (
                    "GITHUB_ENV",
                    github_env.to_str().expect("a UTF-8 GITHUB_ENV"),
                ),
            ]),
        );

        assert!(
            result.is_ok(),
            "a lane failed for the path only the timings upload reads: {result:?}"
        );
        assert!(
            builds.path().join("trivial").join(lease::FILE).is_file(),
            "the lane must still build in its own directory"
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
