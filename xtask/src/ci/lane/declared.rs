use anyhow::Result;

use crate::{ci::process::Process, config::CiLaneConfig};

/// Run a lane the way `.config/xtask.toml` declares it: the platform it refuses
/// to run anywhere but on, the tools it needs, then its commands in order.
///
/// A lane whose whole content is this needs no Rust of its own; the ones that
/// keep a function are the ones that do something a parameter cannot say.
pub(crate) fn run(process: &Process, lane: &CiLaneConfig) -> Result<()> {
    if let Some(os) = lane.os.as_deref() {
        process.require_os(os, &lane.label)?;
    }
    if !lane.tools.is_empty() {
        let tools: Vec<&str> = lane.tools.iter().map(String::as_str).collect();
        process.require_tools(&tools)?;
    }
    for step in &lane.steps {
        let args: Vec<&str> = step.args.iter().map(String::as_str).collect();
        process.run(&lane.program, &args, &step.label)?;
    }
    Ok(())
}
