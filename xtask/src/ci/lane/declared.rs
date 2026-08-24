use std::env;

use anyhow::{Context, Result, bail};
use toml::Value;

use crate::{
    ci::{config::CiPins, process::Process, run::PipelineKind},
    config::{CiLaneConfig, CiLanePin, PIN_PREFIX, ROOT_PLACEHOLDER, SELF_PROGRAM},
};

/// Run a lane the way `.config/xtask.toml` declares it: the pipeline kinds it
/// declines, the platform it refuses to run anywhere but on, the tools it needs,
/// the versions those tools have to report, then its commands in order.
///
/// A lane whose whole content is this needs no Rust of its own; the ones that
/// keep a function are the ones that do something a parameter cannot say.
pub(crate) fn run(
    process: &Process,
    lane: &CiLaneConfig,
    pins: &CiPins,
    kind: PipelineKind,
) -> Result<()> {
    let kind = kind_name(kind);
    if let Some(reason) = lane.kinds_refused.get(&kind) {
        bail!("{reason}");
    }
    if let Some(os) = lane.os.as_deref() {
        process.require_os(os, &lane.label)?;
    }
    if !lane.tools.is_empty() {
        let tools: Vec<&str> = lane.tools.iter().map(String::as_str).collect();
        process.require_tools(&tools)?;
    }
    if !lane.left_behind.is_empty() {
        process.require_left_behind(&lane.left_behind, &lane.left_behind_by)?;
    }
    for check in &lane.pinned {
        require_pinned_version(process, check, pins)?;
    }
    for step in &lane.steps {
        let program = step.program.as_deref().unwrap_or(&lane.program);
        let mut command = if program == SELF_PROGRAM {
            process.command(&env::current_exe().context("locating the running xtask executable")?)
        } else {
            process.command(program)
        };
        let args = step.args_by_kind.get(&kind).unwrap_or(&step.args);
        for arg in args {
            command.arg(resolve(arg, process, pins)?);
        }
        for (key, value) in &step.env {
            command.env(key, resolve(value, process, pins)?);
        }
        process.run_command(&mut command, &step.label)?;
    }
    Ok(())
}

fn kind_name(kind: PipelineKind) -> String {
    clap::ValueEnum::to_possible_value(&kind)
        .map_or_else(|| "unknown".to_owned(), |value| value.get_name().to_owned())
}

/// Fill in the two things a lane cannot spell for itself: where the checkout
/// is, and what a reviewed pin currently holds.
fn resolve(value: &str, process: &Process, pins: &CiPins) -> Result<String> {
    let mut filled = value.replace(ROOT_PLACEHOLDER, &process.root().display().to_string());
    while let Some(start) = filled.find(PIN_PREFIX) {
        let tail = &filled[start + PIN_PREFIX.len()..];
        let end = tail
            .find('}')
            .with_context(|| format!("{PIN_PREFIX} is unclosed in `{value}`"))?;
        let replacement = pin(pins, &tail[..end])?;
        filled.replace_range(start..=start + PIN_PREFIX.len() + end, &replacement);
    }
    Ok(filled)
}

fn require_pinned_version(process: &Process, check: &CiLanePin, pins: &CiPins) -> Result<()> {
    let expected = pin(pins, &check.pin)?;
    let args: Vec<&str> = check.args.iter().map(String::as_str).collect();
    let label = format!("read {} version", check.tool);
    let actual = process.capture(&check.tool, &args, &label)?;
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
