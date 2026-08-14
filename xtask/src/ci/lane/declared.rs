use anyhow::{Result, bail};
use toml::Value;

use crate::{
    ci::{config::CiPins, process::Process},
    config::{CiLaneConfig, CiLanePin},
};

/// Run a lane the way `.config/xtask.toml` declares it: the platform it refuses
/// to run anywhere but on, the tools it needs, the versions those tools have to
/// report, then its commands in order.
///
/// A lane whose whole content is this needs no Rust of its own; the ones that
/// keep a function are the ones that do something a parameter cannot say.
pub(crate) fn run(process: &Process, lane: &CiLaneConfig, pins: &CiPins) -> Result<()> {
    if let Some(os) = lane.os.as_deref() {
        process.require_os(os, &lane.label)?;
    }
    if !lane.tools.is_empty() {
        let tools: Vec<&str> = lane.tools.iter().map(String::as_str).collect();
        process.require_tools(&tools)?;
    }
    for check in &lane.pinned {
        require_pinned_version(process, check, pins)?;
    }
    for step in &lane.steps {
        let args: Vec<&str> = step.args.iter().map(String::as_str).collect();
        process.run(&lane.program, &args, &step.label)?;
    }
    Ok(())
}

fn require_pinned_version(process: &Process, check: &CiLanePin, pins: &CiPins) -> Result<()> {
    let expected = pin(pins, &check.pin)?;
    let args: Vec<&str> = check.args.iter().map(String::as_str).collect();
    let label = format!("read {} version", check.tool);
    let actual = process.capture(&check.tool, &args, &label)?;
    if !actual.split_whitespace().any(|part| part == expected) {
        bail!(
            "{} version mismatch: expected {expected}, got {actual}",
            check.tool
        );
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
