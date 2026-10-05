//! Where Cargo keeps a build directory's units.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use tracing::info;

/// Cargo's profile directories: `<profile>` and `<triple or nested target>/<profile>`,
/// each told by its `.fingerprint`.
pub(super) fn profiles(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut candidates = Vec::new();
    for top in subdirectories(dir)? {
        candidates.extend(subdirectories(&top)?);
        candidates.push(top);
    }
    Ok(candidates
        .into_iter()
        .filter(|candidate| candidate.join(".fingerprint").is_dir())
        .collect())
}

pub(super) fn subdirectories(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("listing {}", dir.display())),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            found.push(entry.path());
        }
    }
    Ok(found)
}

/// Retire only build-script runs whose recorded `OUT_DIR` differs from the
/// physical slot path. Native tools embed that absolute path and cannot reuse
/// a run made through a different checkout's target link.
pub(super) fn retire_relocated_runs(dir: &Path) -> Result<()> {
    let dir = fs::canonicalize(dir)
        .with_context(|| format!("resolving lane build directory {}", dir.display()))?;
    for profile in profiles(&dir)? {
        for run in subdirectories(&profile.join("build"))? {
            let recorded = match fs::read_to_string(run.join("root-output")) {
                Ok(recorded) => recorded,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("reading output root of {}", run.display()));
                }
            };
            if Path::new(recorded.trim()) != run.join("out") {
                info!("retiring relocated build-script run {}", run.display());
                fs::remove_dir_all(&run).with_context(|| {
                    format!("retiring relocated build-script run {}", run.display())
                })?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ci::lane_build::fixture::write_unit;

    #[test]
    fn only_runs_with_a_different_output_path_are_retired() {
        let root = tempfile::tempdir().unwrap();
        let dir = fs::canonicalize(root.path()).unwrap();
        for name in ["relocated", "current", "executable"] {
            let key = format!("debug/build/{name}-0123456789abcdef");
            let output = write_unit(&dir, &key, "cargo:rerun-if-env-changed=CC");
            let run = output.parent().unwrap();
            fs::create_dir_all(run.join("out/build")).unwrap();
            fs::write(run.join("out/build/CMakeCache.txt"), "absolute output path").unwrap();
            match name {
                "relocated" => fs::write(
                    run.join("root-output"),
                    "/old/checkout/target/debug/build/relocated-0123456789abcdef/out",
                )
                .unwrap(),
                "current" => {
                    fs::write(run.join("root-output"), run.join("out").to_str().unwrap()).unwrap();
                }
                _ => {}
            }
        }
        retire_relocated_runs(&dir).unwrap();
        let build = dir.join("debug/build");
        assert!(!build.join("relocated-0123456789abcdef").exists());
        for name in ["current", "executable"] {
            assert!(
                build
                    .join(format!("{name}-0123456789abcdef/out/build/CMakeCache.txt"))
                    .is_file()
            );
        }
    }
}
