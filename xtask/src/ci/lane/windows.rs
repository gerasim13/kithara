use anyhow::Result;

use crate::ci::process::{Process, require_os};

pub(crate) fn tests(process: &Process, target: &str) -> Result<()> {
    require_os("windows", "Windows")?;
    process.require_tools(&["cargo", "rustup", "sccache"])?;
    process.run(
        "rustup",
        &["target", "add", target],
        "install Windows target",
    )?;
    process.run(
        "cargo",
        &["xtask", "test", "--profile", "ci", "--target", target],
        "Windows tests",
    )
}
