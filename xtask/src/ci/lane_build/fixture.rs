//! Checkouts and files the slot claim tests build on.

use std::{
    fs::{self, File},
    path::Path,
    process::Command,
    time::SystemTime,
};

use tempfile::TempDir;

use super::tracked::Sources;

/// Sources from `(path, blob)` pairs.
pub(super) fn sources(entries: &[(&str, &str)]) -> Sources {
    let mut sources = Sources::new();
    for (path, blob) in entries {
        sources
            .entry((*path).to_owned())
            .or_default()
            .insert((*blob).to_owned());
    }
    sources
}

/// Runs git in `dir` against that repository alone: ambient `GIT_*`
/// variables of a hook would point it at the repository running the test.
pub(super) fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_WORK_TREE")
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// A git checkout tracking `files`.
pub(super) fn git_checkout(files: &[(&str, &str)]) -> TempDir {
    let checkout = tempfile::tempdir().unwrap();
    git(checkout.path(), &["init", "-q"]);
    for (path, content) in files {
        let file = checkout.path().join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, content).unwrap();
        git(checkout.path(), &["add", path]);
    }
    checkout
}

pub(super) fn set_mtime(path: &Path, at: SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(at)
        .unwrap();
}

pub(super) fn mtime(path: &Path) -> SystemTime {
    fs::metadata(path).unwrap().modified().unwrap()
}
