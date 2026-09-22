use std::{path::PathBuf, process::Command, time::Duration};

use anyhow::{Result, ensure};
use kithara_devtools::Ctx;

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Compiled SDK fixture containing index.html and generated WebAssembly.
    #[arg(long)]
    directory: PathBuf,
    /// Chrome for Testing headless-shell executable.
    #[arg(long)]
    browser: PathBuf,
}

pub(super) fn run(args: Args, ctx: &Ctx) -> Result<()> {
    let directory = ctx.root.join(args.directory).canonicalize()?;
    ensure!(
        directory.join("index.html").is_file(),
        "SDK fixture has no index.html"
    );
    let browser = ctx.root.join(args.browser).canonicalize()?;
    let cancel = crate::child::Cancel::install()?;
    let status = crate::child::run_bounded(
        Command::new("node")
            .arg(ctx.root.join("tests/crates/ffi-web/sdk/run.mjs"))
            .arg(directory)
            .arg(browser),
        Some(&cancel),
        Duration::from_secs(30),
        1024 * 1024,
    )?;
    ensure!(status.success(), "UniFFI browser contract failed: {status}");
    Ok(())
}
