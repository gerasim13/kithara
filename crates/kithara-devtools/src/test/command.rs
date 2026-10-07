use std::{fmt::Display, path::Path, process::Command};

use anyhow::{Context, Result};
use clap::Args;

use super::{
    request::TestRequest,
    resolve::resolve,
    selection::{requested, select_lane, validate_config},
};
use crate::{
    common::project::{ProjectConfig, TestCommandConfig},
    retried::Evidence,
    touched::{self, Touched},
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
        return run_touched(test, root, &request);
    }
    let lane_name = select_lane(test, &request)?;
    run_lane(test, root, lane_name, &request)
}

/// Run every lane the branch touched.
fn run_touched(test: &TestCommandConfig, root: &Path, request: &TestRequest) -> Result<()> {
    let selected = touched::lanes(test, root, &request.lanes)?;
    if selected.is_empty() {
        println!("no owned path touched; the nightly sweep covers these lanes");
        return Ok(());
    }
    run_each(&selected, |run| {
        let mut command = touched_command(test, run, request)?;
        execute(test, root, run.lane(), &mut command)
    })
}

/// Run `selected` serially without letting the first failure hide the rest:
/// the error names each red lane with its own reason and leaves with the
/// first one's exit code.
pub(super) fn run_each<T, F>(selected: &[T], mut run: F) -> Result<()>
where
    T: Display,
    F: FnMut(&T) -> Result<()>,
{
    let mut failures = Vec::new();
    let mut code = None;
    for lane in selected {
        println!("=== {lane} ===");
        if let Err(error) = run(lane) {
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
fn run_lane(
    test: &TestCommandConfig,
    root: &Path,
    lane_name: &str,
    request: &TestRequest,
) -> Result<()> {
    let mut command = lane_command(test, lane_name, request)?;
    execute(test, root, lane_name, &mut command)
}

/// Runs a lane's command and judges it by its report as well as its status.
pub(super) fn execute(
    test: &TestCommandConfig,
    root: &Path,
    lane_name: &str,
    command: &mut Command,
) -> Result<()> {
    let evidence = Evidence::of(root, test, command)?;
    evidence.clear();
    let status = command.status().with_context(|| {
        format!(
            "failed to run test lane `{lane_name}`: {}",
            command.get_program().to_string_lossy()
        )
    })?;
    evidence.verdict(lane_name, &test.known_flakes, status.code())
}

pub(super) fn lane_command(
    test: &TestCommandConfig,
    lane_name: &str,
    request: &TestRequest,
) -> Result<Command> {
    resolve(test, &requested(test, lane_name, Some(request))?)?
        .command(NextestAction::Run, &request.passthrough)
}

/// The command of one touched run: a lane whole, or narrowed to packages.
pub(super) fn touched_command(
    test: &TestCommandConfig,
    run: &Touched,
    request: &TestRequest,
) -> Result<Command> {
    match run {
        Touched::Whole(lane_name) => lane_command(test, lane_name, request),
        Touched::Narrowed { lane, packages } => {
            resolve(test, &requested(test, lane, Some(request))?)?
                .narrowed(packages)?
                .command(NextestAction::Run, &request.passthrough)
        }
    }
}

/// Operation on the same configured nextest selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextestAction {
    Run,
    List,
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
    resolve(test, &requested(test, lane_name, None)?)?.command(action, extra)
}
