use std::{
    collections::BTreeMap,
    env,
    ffi::{OsStr, OsString},
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use kithara_devtools::common::{project::ProjectConfig, tools::ToolsConfig};
use toml::Value;
use tracing::{info, warn};

use crate::{
    child,
    ci::{
        cache::snapshot, config::CiPins, environment::CacheTrust, lane_build::LaneBuild,
        process::Process, run::PipelineKind,
    },
    config::{CiLaneConfig, CiLanePin, CiLaneStep, LaneFreshness},
    consts,
};

/// Run a lane the way `.config/xtask.toml` declares it: the pipeline kinds it
/// declines, the platform it refuses to run anywhere but on, the tools it needs,
/// the versions those tools have to report, then its commands in order. A step
/// that checks its rebuild is repeated building only, after `claim` replays
/// the claim the next job of this commit would make.
///
/// A lane whose whole content is this needs no Rust of its own; the ones that
/// keep a function are the ones that do something a parameter cannot say.
pub(crate) fn run(
    process: &Process,
    lane: &CiLaneConfig,
    pins: &CiPins,
    project: &ProjectConfig,
    kind: PipelineKind,
    claim: Option<&LaneBuild>,
    test_filter: Option<&str>,
) -> Result<()> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(u64::from(lane.timeout_minutes) * 60))
        .context("declared CI lane deadline exceeds the monotonic clock range")?;
    let kind = kind_name(kind);
    let tools = &project.tools;
    if let Some(expression) = test_filter {
        super::filter::validate(lane, &kind, expression, project)?;
    }
    if let Some(reason) = lane.kinds_refused.get(&kind) {
        bail!("{reason}");
    }
    if let Some(step) = lane.steps.iter().find(|step| step.rebuild_check)
        && claim.is_none()
        && !process.is_recording()
    {
        bail!(
            "{} checks its rebuild by replaying its lane slot's next claim, which only `ci lane` on the fleet hands it",
            step.label
        );
    }
    if !lane.os.is_empty() {
        process.require_os(&lane.os, &lane.label)?;
    }
    if !lane.tools.is_empty() {
        let required: Vec<&str> = lane.tools.iter().map(|role| tools.program(role)).collect();
        process.require_tools(&required)?;
    }
    if !lane.left_behind.is_empty() {
        process.require_left_behind(&lane.left_behind, &lane.left_behind_by)?;
    }
    for check in &lane.pinned {
        require_pinned_version(process, check, pins, tools)?;
    }
    let target_snapshot_to_publish = if process.is_recording() {
        None
    } else {
        lane.target_snapshot
            .as_deref()
            .map(|key| restore_target_layer(process, tools, key))
            .transpose()?
    };
    if !process.is_recording() {
        restore_source_layer(process, tools);
    }
    let cancel = if process.is_recording() {
        None
    } else {
        Some(child::Cancel::install()?)
    };
    for step in &lane.steps {
        let role = step.program.as_deref().unwrap_or(&lane.program);
        let program = if role == consts::SELF_PROGRAM {
            env::current_exe()
                .context("locating the running xtask executable")?
                .into_os_string()
        } else {
            OsString::from(tools.program(role))
        };
        let mut args = step
            .args_by_kind
            .get(&kind)
            .unwrap_or(&step.args)
            .iter()
            .map(|arg| resolve(arg, process, pins))
            .collect::<Result<Vec<_>>>()?;
        if let Some(expression) = test_filter
            && super::filter::apply(role, &mut args, expression, project)?
        {
            info!(step = %step.label, ?program, ?args, "filtered test command");
        }
        let vars = step_vars(lane, step, process, pins)?;
        process.run_command_until(
            process.command(&program).args(&args).envs(&vars),
            &step.label,
            deadline,
            cancel.as_ref(),
        )?;
        if step.rebuild_check {
            rebuild_check(process, claim, &program, &args, &vars, &step.label)?;
        }
    }
    if !process.is_recording() && lane.publishes_sources {
        publish_source_layer(process, tools, &kind)?;
    }
    if let Some(fingerprint) = target_snapshot_to_publish.flatten() {
        let rc = process.resolve_program(tools.program("rc"))?;
        let started = Instant::now();
        let published = snapshot::publish_for_lane(&process.target_dir(), &fingerprint, &rc);
        let took = format!("{:.1} s", started.elapsed().as_secs_f64());
        match published {
            Ok(()) => process.note_cache(
                "target snapshot",
                "published",
                format!("{fingerprint} in {took}"),
            ),
            Err(error) => {
                process.note_cache("target snapshot", "publish failed", format!("{error}"));
                warn!(%error, %fingerprint, "could not publish optional target snapshot");
            }
        }
    }
    Ok(())
}

/// Asks cargo what the next job of this commit would build: replays the claim
/// that job would make, then repeats the step building only, with cargo saying
/// why it builds each unit. The timings report is left out: a build that
/// compiles nothing would still replace the suite's own.
fn rebuild_check(
    process: &Process,
    claim: Option<&LaneBuild>,
    program: &OsStr,
    args: &[String],
    vars: &BTreeMap<String, String>,
    label: &str,
) -> Result<()> {
    if let Some(claim) = claim {
        claim.replay()?;
    }
    let repeat = args
        .iter()
        .map(String::as_str)
        .filter(|arg| !arg.starts_with("--timings"))
        .chain(consts::REBUILD_CHECK_ARGS);
    let label = format!("{label} rebuild check");
    let transcript =
        process.transcript(process.command(program).args(repeat).envs(vars), &label)?;
    let rebuilt = rebuilt(&transcript);
    if !rebuilt.is_empty() {
        bail!(
            "{label}: the next job of this commit would build again:\n{}",
            rebuilt.join("\n")
        );
    }
    Ok(())
}

/// Cargo's status lines for units it would build: `Dirty` says why, and
/// `Compiling` also names a unit it never built.
fn rebuilt(transcript: &str) -> Vec<&str> {
    transcript
        .lines()
        .map(str::trim_start)
        .filter(|line| line.starts_with("Dirty ") || line.starts_with("Compiling "))
        .collect()
}

/// Put the lane's compiled artifacts in place, and say what that cost.
///
/// A fingerprint comes back when nothing was restored: that is the lane which
/// has to publish its own artifacts once the steps pass.
fn restore_target_layer(
    process: &Process,
    tools: &ToolsConfig,
    key: &str,
) -> Result<Option<String>> {
    let rc = process.resolve_program(tools.program("rc"))?;
    let cargo_home = process
        .cargo_home()
        .context("prepared CI environment has no CARGO_HOME")?;
    let started = Instant::now();
    let outcome =
        snapshot::restore_for_lane(key, &process.target_dir(), process.root(), &cargo_home, &rc);
    let took = format!("{:.1} s", started.elapsed().as_secs_f64());
    match &outcome {
        Ok(None) => {
            process.note_cache(
                "target snapshot",
                "restored",
                format!("{key} unpacked in {took}"),
            );
        }
        Ok(Some(fingerprint)) => {
            process.note_cache(
                "target snapshot",
                "miss",
                format!(
                    "no object for {fingerprint}; this lane will publish one (asked in {took})"
                ),
            );
        }
        Err(error) => {
            process.note_cache("target snapshot", "unavailable", format!("{error}"));
        }
    }
    outcome
}

fn kind_name(kind: PipelineKind) -> String {
    kind.name().to_owned()
}

/// Put the dependency sources in place before the lane's first Cargo command.
///
/// A failed restore stays non-fatal: the layer is an accelerator, and a lane
/// that cannot reach the cache must still be able to fetch and run. What it no
/// longer does is report every outcome as the same warning. A lane whose
/// `Cargo.lock` has no object yet is the ordinary case and says so; only a
/// lane that could not ask warns.
fn restore_source_layer(process: &Process, tools: &ToolsConfig) {
    let Some((cargo_home, rc)) = source_layer_access(process, tools) else {
        return;
    };
    let started = Instant::now();
    let outcome = snapshot::restore_sources(process.root(), &cargo_home, &rc);
    let took = format!("{:.1} s", started.elapsed().as_secs_f64());
    match outcome {
        Ok(snapshot::Restored::AlreadyPresent) => {
            process.note_cache(
                "dependency sources",
                "reused",
                format!("already in place for this lock file, checked in {took}"),
            );
        }
        Ok(snapshot::Restored::Absent) => {
            process.note_cache(
                "dependency sources",
                "miss",
                format!("no layer for this lock file yet; the lane will fetch (asked in {took})"),
            );
        }
        Ok(snapshot::Restored::Layer(object)) => {
            process.note_cache(
                "dependency sources",
                "restored",
                format!("{object} in {took}"),
            );
        }
        Err(error) => {
            process.note_cache("dependency sources", "unavailable", format!("{error}"));
            warn!(%error, "could not restore the dependency sources");
        }
    }
}

/// Record what the lane ended up with, once its steps have fetched whatever
/// the restored layer did not carry. Only the default branch in the trusted
/// scope publishes, because that scope is the only one the bucket policy lets
/// write.
///
/// A publisher's failure fails the lane. The defect this repairs is that it did
/// not: the publish was refused on every run for weeks and the lane stayed
/// green, so the layer read as a working cache that happened to always miss.
fn publish_source_layer(process: &Process, tools: &ToolsConfig, kind: &str) -> Result<()> {
    if !is_source_publisher(kind, CacheTrust::from_environment()?) {
        return Ok(());
    }
    let Some((cargo_home, rc)) = source_layer_access(process, tools) else {
        return Ok(());
    };
    let started = Instant::now();
    let published = snapshot::publish_sources(process.root(), &cargo_home, &rc)
        .context("publish the dependency sources");
    let took = format!("{:.1} s", started.elapsed().as_secs_f64());
    match &published {
        Ok(()) => {
            process.note_cache("dependency sources", "published", format!("in {took}"));
        }
        Err(error) => {
            process.note_cache("dependency sources", "publish failed", format!("{error}"));
        }
    }
    published
}

/// A default-branch run in another scope holds that scope's keys: a fork's
/// `main` runs as `review`, and its publish was refused on every push.
fn is_source_publisher(kind: &str, trust: CacheTrust) -> bool {
    kind == PipelineKind::Main.name() && trust == CacheTrust::Trusted
}

fn source_layer_access(process: &Process, tools: &ToolsConfig) -> Option<(PathBuf, PathBuf)> {
    let cargo_home = process.cargo_home()?;
    match process.resolve_program(tools.program("rc")) {
        Ok(rc) => Some((cargo_home, rc)),
        Err(error) => {
            warn!(%error, "no cache client; the lane carries its own sources");
            None
        }
    }
}

/// Fill in the things a lane cannot spell for itself: where the checkout is,
/// where its leased build cache lives, and what a reviewed pin currently
/// holds.
fn resolve(value: &str, process: &Process, pins: &CiPins) -> Result<String> {
    let mut filled = value
        .replace(
            consts::ROOT_PLACEHOLDER,
            &process.root().display().to_string(),
        )
        .replace(
            consts::TARGET_PLACEHOLDER,
            &process.target_dir().display().to_string(),
        );
    while let Some(start) = filled.find(consts::PIN_PREFIX) {
        let tail = &filled[start + consts::PIN_PREFIX.len()..];
        let end = tail.find('}').with_context(|| {
            format!(
                "{PIN_PREFIX} is unclosed in `{value}`",
                PIN_PREFIX = consts::PIN_PREFIX
            )
        })?;
        let replacement = pin(pins, &tail[..end])?;
        filled.replace_range(start..=start + consts::PIN_PREFIX.len() + end, &replacement);
    }
    Ok(filled)
}

/// The step's own variables, and for a checksum lane the two that make cargo
/// judge the lane's directory by checksum: the flag and the nightly that
/// honours it.
fn step_vars(
    lane: &CiLaneConfig,
    step: &CiLaneStep,
    process: &Process,
    pins: &CiPins,
) -> Result<BTreeMap<String, String>> {
    let mut vars = step
        .env
        .iter()
        .map(|(key, value)| Ok((key.clone(), resolve(value, process, pins)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    if lane.freshness == LaneFreshness::Checksum {
        vars.insert(consts::CHECKSUM_FRESHNESS_ENV.to_owned(), "true".to_owned());
        vars.insert(
            consts::TOOLCHAIN_ENV.to_owned(),
            pins.nightly_toolchain.clone(),
        );
    }
    Ok(vars)
}

fn require_pinned_version(
    process: &Process,
    check: &CiLanePin,
    pins: &CiPins,
    tools: &ToolsConfig,
) -> Result<()> {
    let expected = pin(pins, &check.pin)?;
    let args: Vec<&str> = check.args.iter().map(String::as_str).collect();
    let label = format!("read {} version", check.tool);
    let actual = process.capture(tools.program(&check.tool), &args, &label)?;
    let Some(prefix) = check.line_prefix.as_deref() else {
        if !actual.split_whitespace().any(|part| part == expected) {
            bail!(
                "{} version mismatch: expected {expected}, got {actual}",
                check.tool
            );
        }
        return Ok(());
    };
    let reported = actual
        .lines()
        .next()
        .and_then(|line| line.strip_prefix(prefix))
        .with_context(|| format!("{} did not report a version", check.tool))?;
    if reported != expected {
        bail!("{prefix}{expected} is required, found {reported}");
    }
    Ok(())
}

/// Pins are reviewed as data, so a lane names the one it wants rather than
/// reaching for a field: a new pin costs a line in `.config/ci-pins.toml`.
fn pin(pins: &CiPins, key: &str) -> Result<String> {
    let table = Value::try_from(pins)?;
    match table.get(key) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(other) => bail!(
            "pin {key} is {} rather than a version string",
            other.type_str()
        ),
        None => bail!("{key} is not a pin in .config/ci-pins.toml"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::ci::{
        config::fixture,
        process::{Recording, Step},
    };

    /// A lane of the given freshness whose one step runs the suite into
    /// `{target}/suite`.
    fn checksum_lane(freshness: LaneFreshness) -> CiLaneConfig {
        CiLaneConfig {
            label: "fixture".to_owned(),
            program: "just".to_owned(),
            freshness,
            steps: vec![CiLaneStep {
                args: vec!["test".to_owned(), "run".to_owned()],
                label: "suite".to_owned(),
                env: BTreeMap::from([("CARGO_TARGET_DIR".to_owned(), "{target}/suite".to_owned())]),
                ..CiLaneStep::default()
            }],
            ..CiLaneConfig::default()
        }
    }

    /// The steps a lane asks for, recorded against a checkout at `/checkout`.
    fn recorded(lane: &CiLaneConfig) -> Vec<Step> {
        let process = Process::recording(Path::new("/checkout"), Recording::default());
        run(
            &process,
            lane,
            &fixture().pins,
            &ProjectConfig::default(),
            PipelineKind::Branch,
            None,
            None,
        )
        .unwrap();
        process.recorded().unwrap().steps().to_vec()
    }

    /// Checksum freshness is honoured only by nightly cargo, so the lane that
    /// asks for it gets the flag and the pinned nightly together.
    #[test]
    fn a_checksum_lane_builds_with_the_pinned_nightly() {
        let steps = recorded(&checksum_lane(LaneFreshness::Checksum));

        let env = &steps[0].env;
        assert_eq!(
            env.get(consts::CHECKSUM_FRESHNESS_ENV).map(String::as_str),
            Some("true")
        );
        assert_eq!(
            env.get(consts::TOOLCHAIN_ENV),
            Some(&fixture().pins.nightly_toolchain)
        );
        assert_eq!(
            env.get("CARGO_TARGET_DIR").map(String::as_str),
            Some("/checkout/target/suite")
        );
    }

    #[test]
    fn an_mtime_lane_leaves_the_toolchain_to_the_step() {
        let steps = recorded(&checksum_lane(LaneFreshness::Mtime));

        let env = &steps[0].env;
        assert!(!env.contains_key(consts::CHECKSUM_FRESHNESS_ENV), "{env:?}");
        assert!(!env.contains_key(consts::TOOLCHAIN_ENV), "{env:?}");
    }

    /// The check repeats its step with the same variables, building only, with
    /// cargo saying in plain text why it builds each unit, and without the
    /// timings report a build that compiles nothing would still rewrite.
    #[test]
    fn a_rebuild_check_repeats_its_step_building_only() {
        let mut lane = checksum_lane(LaneFreshness::Checksum);
        lane.steps[0].args.push("--timings".to_owned());
        lane.steps[0].rebuild_check = true;

        let steps = recorded(&lane);

        let [suite, check] = steps.as_slice() else {
            panic!("the suite and its check: {steps:?}")
        };
        assert_eq!(check.label, "suite rebuild check");
        assert_eq!(
            check.args,
            [
                "test",
                "run",
                "--no-run",
                "--cargo-verbose",
                "--color",
                "never"
            ]
        );
        assert_eq!(check.env, suite.env);
    }

    /// Only `ci lane` on the fleet hands the check the claim it replays; any
    /// other run refuses before its suite spends a build.
    #[test]
    fn a_rebuild_check_without_a_slot_refuses_before_the_suite_runs() {
        let mut lane = checksum_lane(LaneFreshness::Checksum);
        lane.steps[0].rebuild_check = true;
        let process = Process::new(Path::new("/checkout"), BTreeMap::new());

        let error = run(
            &process,
            &lane,
            &fixture().pins,
            &ProjectConfig::default(),
            PipelineKind::Branch,
            None,
            None,
        )
        .expect_err("no slot to replay");

        assert!(error.to_string().contains("ci lane"), "{error}");
    }

    #[test]
    fn a_rebuild_is_read_from_cargos_status_lines() {
        let transcript = "       Fresh serde v1.0.0\n       Dirty probe v0.0.0 (/w): the file `src/lib.rs` has changed\n   Compiling probe v0.0.0 (/w)\n     Running `rustc --crate-name probe`\nwarning: Compiling is mentioned, not reported\n";

        assert_eq!(
            rebuilt(transcript),
            [
                "Dirty probe v0.0.0 (/w): the file `src/lib.rs` has changed",
                "Compiling probe v0.0.0 (/w)",
            ]
        );
    }

    #[test]
    fn only_the_trusted_default_branch_publishes_the_source_layer() {
        let main = PipelineKind::Main.name();

        assert!(is_source_publisher(main, CacheTrust::Trusted));
        assert!(!is_source_publisher(main, CacheTrust::Review));
        assert!(!is_source_publisher(main, CacheTrust::Quarantine));
        assert!(!is_source_publisher(
            PipelineKind::Branch.name(),
            CacheTrust::Trusted
        ));
    }
}
