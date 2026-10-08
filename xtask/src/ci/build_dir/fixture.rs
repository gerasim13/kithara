//! Checkouts the build-directory tests claim.

use std::{fs, path::Path, process::Command};

use tempfile::TempDir;

/// Runs git in `dir` against that repository alone: ambient `GIT_*`
/// variables of a hook would point it at the repository running the test.
pub(crate) fn git(dir: &Path, args: &[&str]) {
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
pub(crate) fn git_checkout(files: &[(&str, &str)]) -> TempDir {
    let checkout = tempfile::tempdir().unwrap();
    git(checkout.path(), &["init", "-q"]);
    for (path, content) in files {
        let file = checkout.path().join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, content).unwrap();
    }
    git(checkout.path(), &["add", "-A"]);
    checkout
}
