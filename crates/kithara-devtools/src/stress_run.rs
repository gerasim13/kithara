//! Runs a controller-defined nextest stress campaign against the current workspace.

use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, ensure};
use clap::{ArgAction, Args};

use crate::{
    Ctx,
    stress_report::{MAX_INVENTORY_BYTES, read_bounded_utf8, validate_inventory},
    test::{LaneToggles, NextestAction, nextest_lane_command_for},
    verdict::ChildFailure,
};

const MAX_STRESS_COUNT: usize = 100;
const MAX_TEST_THREADS: usize = 256;

#[derive(Debug, Args)]
#[non_exhaustive]
pub struct StressRunArgs {
    /// Machine-readable inventory to write before executing the selection.
    #[arg(long)]
    inventory: PathBuf,
    /// Reviewed nextest configuration used for listing and running the subject.
    #[arg(long)]
    config_file: PathBuf,
    /// Nextest filterset selecting the tests to repeat.
    #[arg(long)]
    filter: String,
    /// Number of times to execute every selected test.
    #[arg(long)]
    count: usize,
    /// Nextest runner concurrency (`num-cpus` or a positive integer).
    #[arg(long, default_value = "num-cpus")]
    test_threads: String,
    /// Compile and run the subject with Flash enabled.
    #[arg(long, action = ArgAction::Set)]
    flash: bool,
    /// Compile and run the subject with no-block instrumentation enabled.
    #[arg(long, action = ArgAction::Set)]
    no_block: bool,
}

/// Lists the exact selection, validates it, then runs every selected test.
///
/// # Errors
///
/// Returns an error when the campaign inputs are invalid, listing does not
/// produce a usable inventory, or nextest cannot complete the stress run.
pub(crate) fn run(args: &StressRunArgs, ctx: &Ctx) -> Result<()> {
    validate_stress_count(args.count)?;
    ensure!(!args.filter.trim().is_empty(), "stress filter is empty");
    validate_test_threads(&args.test_threads)?;
    ensure!(
        args.config_file.is_file(),
        "nextest config does not exist: {}",
        args.config_file.display()
    );
    let config_file = args
        .config_file
        .to_str()
        .context("nextest config path is not UTF-8")?;
    let toggles = LaneToggles {
        flash: args.flash,
        no_block: args.no_block,
    };
    let backend = &ctx.config.test.default_backend;

    let inventory_args = vec![
        "--profile".to_owned(),
        "stress".to_owned(),
        "--config-file".to_owned(),
        config_file.to_owned(),
        "-E".to_owned(),
        args.filter.clone(),
        "--message-format".to_owned(),
        "json".to_owned(),
    ];
    let (_, mut inventory) = nextest_lane_command_for(
        &ctx.config,
        toggles,
        backend,
        &inventory_args,
        NextestAction::List,
    )?;
    let output = create_inventory(&args.inventory)?;
    inventory
        .current_dir(&ctx.root)
        .stdout(Stdio::from(output))
        .stderr(Stdio::inherit());
    let status = inventory.status().context("start nextest inventory")?;
    if !status.success() {
        return Err(ChildFailure::inherited(
            "nextest inventory".to_owned(),
            status.code(),
        ));
    }
    let json = read_bounded_utf8(&args.inventory, MAX_INVENTORY_BYTES, "stress inventory")?;
    validate_inventory(&json).context("validate nextest inventory")?;

    let run_args = vec![
        "--profile".to_owned(),
        "stress".to_owned(),
        "--config-file".to_owned(),
        config_file.to_owned(),
        "-E".to_owned(),
        args.filter.clone(),
        "--stress-count".to_owned(),
        args.count.to_string(),
        "--test-threads".to_owned(),
        args.test_threads.clone(),
    ];
    let (_, mut run) =
        nextest_lane_command_for(&ctx.config, toggles, backend, &run_args, NextestAction::Run)?;
    run.current_dir(&ctx.root);
    run_child(&mut run)
}

fn validate_test_threads(value: &str) -> Result<()> {
    if value == "num-cpus" {
        return Ok(());
    }
    let count = value
        .parse::<usize>()
        .with_context(|| format!("invalid test-threads value `{value}`"))?;
    ensure!(count > 0, "test-threads must be greater than zero");
    ensure!(
        count <= MAX_TEST_THREADS,
        "test-threads must not exceed {MAX_TEST_THREADS}"
    );
    Ok(())
}

fn validate_stress_count(count: usize) -> Result<()> {
    ensure!(count > 0, "stress count must be greater than zero");
    ensure!(
        count <= MAX_STRESS_COUNT,
        "stress count must not exceed {MAX_STRESS_COUNT}"
    );
    Ok(())
}

fn create_inventory(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create stress inventory directory {}", parent.display()))?;
    }
    File::create(path).with_context(|| format!("create stress inventory {}", path.display()))
}

fn run_child(command: &mut Command) -> Result<()> {
    let status = command.status().context("start nextest stress run")?;
    if status.success() {
        return Ok(());
    }
    Err(ChildFailure::inherited(
        "nextest stress run".to_owned(),
        status.code(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_threads_accepts_nextest_concurrency_values() {
        for value in ["num-cpus", "1", "32"] {
            validate_test_threads(value).expect("valid test thread count");
        }
    }

    #[test]
    fn test_threads_rejects_invalid_or_unsupported_values() {
        for value in ["", "0", "257", "all"] {
            assert!(validate_test_threads(value).is_err(), "{value}");
        }
    }

    #[test]
    fn stress_count_is_limited_to_supported_campaign_sizes() {
        for count in [1, 50, 100] {
            validate_stress_count(count).expect("valid stress count");
        }
        for count in [0, 101, usize::MAX] {
            assert!(validate_stress_count(count).is_err(), "{count}");
        }
    }
}
