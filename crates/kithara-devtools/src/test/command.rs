use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    process::Command,
};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde::{Deserialize, Serialize};

use super::{
    request::TestRequest,
    selection::{
        LaneToggles, PassthroughPosition, backend_name, features_for, lane_features, lane_toggles,
        passthrough_position, select_lane, validate_config,
    },
};
use crate::{
    common::project::{ProjectConfig, TestCommandConfig, TestLaneConfig},
    retried::Evidence,
    touched,
    verdict::ChildFailure,
};

#[derive(Debug, Args)]
#[command(trailing_var_arg = true)]
pub struct TestArgs {
    /// Arguments for the configured test command. Recipe-level flags accepted anywhere:
    /// `--lane=<configured-name>`, `--touched`, `--flash=true|false|on|off`, `--no-flash`,
    /// `--loom=true|false|on|off`, `--no-loom`, `--no-block=true|false|on|off`, and
    /// `--net-backend=<configured-name>`.
    #[arg(value_name = "ARGS", allow_hyphen_values = true)]
    pub(crate) args: Vec<String>,
}

pub(crate) fn run(args: &TestArgs) -> Result<()> {
    /// Where the test command is run from, and so where every path it reads
    /// or judges is anchored.
    const ROOT: &str = ".";

    let request = TestRequest::parse(&args.args)?;
    let root = Path::new(ROOT);
    let project = ProjectConfig::load(root)?;
    let test = &project.test;
    validate_config(test)?;

    if request.touched {
        return run_touched(&project, root, &request);
    }
    let (lane_name, lane) = select_lane(test, &request)?;
    run_lane(&project, root, lane_name, lane, &request)
}

/// Run every lane the branch touched.
fn run_touched(project: &ProjectConfig, root: &Path, request: &TestRequest) -> Result<()> {
    let selected = touched::lanes(&project.test, &request.lanes)?;
    if selected.is_empty() {
        println!("no owned path touched; the nightly sweep covers these lanes");
        return Ok(());
    }
    run_each(project, root, request, &selected)
}

/// Run `selected` serially without letting the first failure hide the rest:
/// the error names each red lane with its own reason and leaves with the
/// first one's exit code.
pub(super) fn run_each(
    project: &ProjectConfig,
    root: &Path,
    request: &TestRequest,
    selected: &[String],
) -> Result<()> {
    let mut failures = Vec::new();
    let mut code = None;
    for lane_name in selected {
        let lane = project
            .test
            .lanes
            .get(lane_name)
            .with_context(|| format!("test lane `{lane_name}` is not configured"))?;
        println!("=== {lane_name} ===");
        if let Err(error) = run_lane(project, root, lane_name, lane, request) {
            code.get_or_insert_with(|| {
                error
                    .downcast_ref::<ChildFailure>()
                    .map_or(1, ChildFailure::exit_code)
            });
            failures.push(format!("{error:#}"));
        }
    }
    code.map_or(Ok(()), |code| {
        Err(ChildFailure::explained(
            "touched test lanes".to_owned(),
            Some(code),
            failures.join("\n"),
        ))
    })
}

/// Reports build time before the verdict rather than after it: a red lane is exactly when the
/// build's share of the wall clock needs explaining, and reporting after an early return would
/// print the number only for lanes that passed.
pub(super) fn run_lane(
    project: &ProjectConfig,
    root: &Path,
    lane_name: &str,
    lane: &TestLaneConfig,
    request: &TestRequest,
) -> Result<()> {
    let mut cmd = lane_command(project, lane_name, lane, request)?;
    let evidence = Evidence::of(root, &project.test, &cmd)?;
    evidence.clear();

    let status = cmd
        .status()
        .with_context(|| format!("failed to run test lane `{lane_name}`: {}", lane.program))?;
    evidence.verdict(lane_name, &project.test.known_flakes, status.code())
}

pub(super) fn lane_command(
    project: &ProjectConfig,
    lane_name: &str,
    lane: &TestLaneConfig,
    request: &TestRequest,
) -> Result<Command> {
    let test = &project.test;
    let passthrough = passthrough_position(lane)?;
    match passthrough {
        PassthroughPosition::BeforeSuffix if lane_name == test.default_lane => {
            let toggles = lane_toggles(test, lane, Some(request));
            let backend = backend_name(test, lane, request);
            let (_, cmd) = nextest_lane_command(project, toggles, backend, &request.passthrough)?;
            Ok(cmd)
        }
        passthrough => {
            let mut cmd = Command::new(&lane.program);
            cmd.envs(&lane.env);
            cmd.args(&lane.prefix_args);
            let features = features_for(test, lane, request)?;
            if !features.is_empty() {
                cmd.arg(&test.feature_arg)
                    .arg(features.into_iter().collect::<Vec<_>>().join(","));
            }
            let caller_args = caller_args_for(lane, &request.passthrough);
            match passthrough {
                PassthroughPosition::BeforeSuffix => {
                    cmd.args(&caller_args);
                    cmd.args(&lane.suffix_args);
                }
                PassthroughPosition::AfterSuffix => {
                    cmd.args(&lane.suffix_args);
                    cmd.args(&caller_args);
                }
            }
            Ok(cmd)
        }
    }
}

/// The caller's arguments as a named lane receives them.
///
/// A lane that names its own nextest profile keeps it: the profile carries the
/// lane's filter, and nextest refuses a second `--profile` outright. A gate
/// that fans `--touched` out over such lanes passes the profile the default
/// lane needs, so every lane of that fan-out would otherwise fail before a
/// single test ran.
fn caller_args_for(lane: &TestLaneConfig, passthrough: &[String]) -> Vec<String> {
    let names_profile = lane
        .prefix_args
        .iter()
        .any(|arg| arg == "--profile" || arg.starts_with("--profile="));
    if !names_profile {
        return passthrough.to_vec();
    }
    let mut args = Vec::with_capacity(passthrough.len());
    let mut iter = passthrough.iter();
    while let Some(arg) = iter.next() {
        if arg == "--profile" {
            iter.next();
        } else if !arg.starts_with("--profile=") {
            args.push(arg.clone());
        }
    }
    args
}

pub(crate) fn nextest_lane_command(
    project: &ProjectConfig,
    toggles: LaneToggles,
    backend: &str,
    extra: &[String],
) -> Result<(Vec<String>, Command)> {
    nextest_lane_command_for(project, toggles, backend, extra, NextestAction::Run)
}

/// Operation on the same configured nextest selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextestAction {
    Run,
    List,
}

/// Build the default suite command for a platform adapter.
///
/// # Errors
/// Returns an error when the configured suite or backend is invalid.
pub fn default_nextest_command(
    project: &ProjectConfig,
    extra: &[String],
    action: NextestAction,
) -> Result<Command> {
    nextest_command_for_lane(project, &project.test.default_lane, extra, action)
}

/// Build the command for a configured test lane.
/// # Errors
/// Returns an error when the lane or its backend configuration is invalid.
pub fn nextest_command_for_lane(
    project: &ProjectConfig,
    lane_name: &str,
    extra: &[String],
    action: NextestAction,
) -> Result<Command> {
    let test = &project.test;
    validate_config(test)?;
    let lane = test
        .lanes
        .get(lane_name)
        .with_context(|| format!("test lane `{lane_name}` is not configured"))?;
    let toggles = lane_toggles(test, lane, None);
    let backend = lane
        .default_backend
        .as_deref()
        .unwrap_or(&test.default_backend);
    let features = lane_features(test, lane, toggles, backend)?;
    let (_, command) = nextest_command(test, lane, features, extra, action)?;
    Ok(command)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfiguredLane {
    /// A lane with no environment of its own records none, so a campaign that
    /// predates lane environments keeps comparing against the same runner.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) backend: String,
    pub(crate) feature_arg: String,
    pub(crate) lane: String,
    pub(crate) program: String,
    pub(crate) features: Vec<String>,
    pub(crate) prefix_args: Vec<String>,
    pub(crate) suffix_args: Vec<String>,
}

pub(crate) fn nextest_lane_command_for(
    project: &ProjectConfig,
    toggles: LaneToggles,
    backend: &str,
    extra: &[String],
    action: NextestAction,
) -> Result<(Vec<String>, Command)> {
    let test = &project.test;
    validate_config(test)?;
    let lane_name = &test.default_lane;
    let Some(lane) = test.lanes.get(lane_name) else {
        bail!("test.default_lane `{lane_name}` is not defined in test.lanes");
    };
    let features = lane_features(test, lane, toggles, backend)?;
    nextest_command(test, lane, features, extra, action)
}

pub(crate) fn nextest_configured_lane_command(
    resolved: &ConfiguredLane,
    extra: &[String],
    action: NextestAction,
) -> Result<(Vec<String>, Command)> {
    resolved.validate()?;
    build_nextest_command(
        NextestSpec {
            program: &resolved.program,
            prefix_args: &resolved.prefix_args,
            suffix_args: &resolved.suffix_args,
            feature_arg: &resolved.feature_arg,
            env: &resolved.env,
        },
        resolved.features.iter().cloned().collect(),
        extra,
        action,
    )
}

pub(crate) fn configured_lane(
    project: &ProjectConfig,
    lane_name: &str,
    backend_name: &str,
    additional_features: &[String],
) -> Result<ConfiguredLane> {
    let test = &project.test;
    validate_config(test)?;
    let lane = test
        .lanes
        .get(lane_name)
        .with_context(|| format!("stress lane `{lane_name}` is not configured"))?;
    let mut features = BTreeSet::new();
    features.extend(test.features.iter().cloned());
    features.extend(lane.default_features.iter().cloned());
    features.extend(additional_features.iter().cloned());
    let backend = test
        .net_backends
        .get(backend_name)
        .with_context(|| format!("stress backend `{backend_name}` is not configured"))?;
    features.extend(backend.features.iter().cloned());
    Ok(ConfiguredLane {
        lane: lane_name.to_owned(),
        backend: backend_name.to_owned(),
        program: lane.program.clone(),
        prefix_args: lane.prefix_args.clone(),
        suffix_args: lane.suffix_args.clone(),
        feature_arg: test.feature_arg.clone(),
        features: features.into_iter().collect(),
        env: lane.env.clone(),
    })
}

impl ConfiguredLane {
    pub(crate) fn validate(&self) -> Result<()> {
        for (field, value) in [
            ("lane", self.lane.as_str()),
            ("backend", self.backend.as_str()),
            ("program", self.program.as_str()),
            ("feature argument", self.feature_arg.as_str()),
        ] {
            if value.trim().is_empty() {
                bail!("configured test runner {field} is empty");
            }
        }
        let mut features = BTreeSet::new();
        for feature in &self.features {
            if feature.trim().is_empty() {
                bail!("configured test runner contains an empty feature");
            }
            if !features.insert(feature) {
                bail!("configured test runner contains duplicate feature `{feature}`");
            }
        }
        Ok(())
    }
}

/// What a lane runs, resolved from either the lane config or a configured
/// runner recorded by a campaign.
#[derive(Clone, Copy)]
struct NextestSpec<'a> {
    env: &'a BTreeMap<String, String>,
    prefix_args: &'a [String],
    suffix_args: &'a [String],
    feature_arg: &'a str,
    program: &'a str,
}

fn nextest_command(
    test: &TestCommandConfig,
    lane: &TestLaneConfig,
    features: BTreeSet<String>,
    extra: &[String],
    action: NextestAction,
) -> Result<(Vec<String>, Command)> {
    build_nextest_command(
        NextestSpec {
            program: &lane.program,
            prefix_args: &lane.prefix_args,
            suffix_args: &lane.suffix_args,
            feature_arg: &test.feature_arg,
            env: &lane.env,
        },
        features,
        extra,
        action,
    )
}

fn build_nextest_command(
    spec: NextestSpec<'_>,
    features: BTreeSet<String>,
    extra: &[String],
    action: NextestAction,
) -> Result<(Vec<String>, Command)> {
    let NextestSpec {
        program,
        prefix_args,
        suffix_args,
        feature_arg,
        env,
    } = spec;
    let mut cmd = Command::new(program);
    cmd.envs(env);
    let mut prefix_args = prefix_args.to_vec();
    if extra
        .iter()
        .any(|arg| matches!(arg.as_str(), "-p" | "--package") || arg.starts_with("--package="))
    {
        let mut skip = false;
        prefix_args.retain(|arg| {
            if skip {
                skip = false;
                return false;
            }
            if arg == "--exclude" {
                skip = true;
                return false;
            }
            arg != "--workspace" && !arg.starts_with("--exclude=")
        });
    }
    match action {
        NextestAction::Run => {
            cmd.args(&prefix_args);
        }
        NextestAction::List => {
            let nextest_index = prefix_args
                .iter()
                .position(|arg| arg == "nextest")
                .context("default test lane must contain `nextest` for list inventory")?;
            let action_index = prefix_args
                .iter()
                .enumerate()
                .skip(nextest_index + 1)
                .find_map(|(index, arg)| (arg == "run").then_some(index))
                .context("default test lane must contain a `run` action after `nextest`")?;
            prefix_args[action_index] = "list".to_owned();
            cmd.args(prefix_args);
        }
    }
    if !features.is_empty() {
        cmd.arg(feature_arg)
            .arg(features.iter().cloned().collect::<Vec<_>>().join(","));
    }
    cmd.args(extra);
    cmd.args(suffix_args);
    Ok((features.into_iter().collect(), cmd))
}
