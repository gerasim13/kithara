use anyhow::Result;

use crate::ci::process::{Process, require_os};

fn preflight(process: &Process) -> Result<()> {
    require_os("linux", "dependency audit")?;
    process.require_tools(&["cargo", "just"])
}

pub(crate) fn deny(process: &Process) -> Result<()> {
    preflight(process)?;
    process.run("just", &["deps", "deny"], "licence and advisory gate")
}

pub(crate) fn unused(process: &Process) -> Result<()> {
    preflight(process)?;
    process.run("just", &["deps", "machete"], "unused manifest entries")?;
    process.run("just", &["deps", "shear"], "unused crate dependencies")
}

pub(crate) fn features(process: &Process) -> Result<()> {
    preflight(process)?;
    process.run("just", &["deps", "hack"], "feature powerset check")
}

pub(crate) fn semver(process: &Process) -> Result<()> {
    preflight(process)?;
    process.run("just", &["deps", "semver"], "public API compatibility")
}
