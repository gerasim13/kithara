//! Where Cargo keeps a build directory's units.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

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
