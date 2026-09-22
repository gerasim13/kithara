use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context as _, Result, ensure};
use clap::Subcommand;
use kithara_devtools::Ctx;
use serde::Serialize;
use tracing::info;

use super::discover::{Declaration, discover};

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCommand {
    /// Emit syntactic candidates; this does not certify SDK coverage.
    Discover {
        /// JSON output, relative to the workspace unless absolute.
        #[arg(long, default_value = "target/config-protocol/discovery.json")]
        output: PathBuf,
    },
}

#[derive(Serialize)]
struct Inventory {
    schema_version: u32,
    rust_files: usize,
    declarations: Vec<Declaration>,
}

pub(crate) fn run(command: ConfigCommand, ctx: &Ctx) -> Result<()> {
    let ConfigCommand::Discover { output } = command;
    let inventory = inventory(&ctx.root)?;
    let output = ctx.root.join(output);
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut json = serde_json::to_string_pretty(&inventory)?;
    json.push('\n');
    fs::write(&output, json).with_context(|| format!("write {}", output.display()))?;
    info!(files = inventory.rust_files, candidates = inventory.declarations.len(), path = %output.display(), "configuration discovery complete; semantic classification is not checked");
    Ok(())
}

fn inventory(root: &Path) -> Result<Inventory> {
    // Include new source files without scanning ignored build outputs.
    let files = Command::new("git")
        .current_dir(root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .output()
        .context("list configuration discovery inputs")?;
    ensure!(
        files.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&files.stderr)
    );
    let paths = String::from_utf8(files.stdout).context("non-UTF-8 discovery path")?;
    let mut paths: Vec<_> = paths
        .split('\0')
        .filter(|path| path.ends_with(".rs"))
        .collect();
    paths.sort_unstable();
    paths.dedup();
    ensure!(!paths.is_empty(), "no Rust discovery inputs");
    let mut inventory = Inventory {
        schema_version: 1,
        rust_files: paths.len(),
        declarations: Vec::new(),
    };
    for path in paths {
        let source = fs::read_to_string(root.join(path))
            .with_context(|| format!("read configuration discovery input {path}"))?;
        inventory.declarations.extend(discover(path, &source)?);
    }
    Ok(inventory)
}

#[cfg(test)]
mod tests {
    use std::{fs, process::Command};

    use super::inventory;

    #[test]
    fn discovery_includes_untracked_source_excludes_artifacts_and_rejects_bad_source() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "--quiet"])
                .arg(root.path())
                .status()
                .unwrap()
                .success()
        );
        fs::write(root.path().join(".gitignore"), "target/\n").unwrap();
        fs::create_dir(root.path().join("target")).unwrap();
        fs::write(root.path().join("target/generated.rs"), "not valid Rust").unwrap();
        fs::write(
            root.path().join("lib.rs"),
            "struct NewConfig { value: u64 }",
        )
        .unwrap();
        let first = inventory(root.path()).unwrap();
        assert_eq!(first.rust_files, 1);
        assert_eq!(first.declarations.len(), 1);
        assert_eq!(
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&inventory(root.path()).unwrap()).unwrap()
        );
        fs::write(root.path().join("broken.rs"), "struct MissingConfig {").unwrap();
        assert!(inventory(root.path()).is_err());
    }
}
