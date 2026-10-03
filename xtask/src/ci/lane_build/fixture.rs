//! Checkouts and files the slot claim tests build on.

use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

use tempfile::TempDir;

use super::{pool::SlotPool, tracked::Sources};
use crate::{ci::environment::CacheTrust, config::LaneFreshness, consts};

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

/// A git checkout of the one-package workspace `probe`, plus `files`.
pub(super) fn cargo_checkout(files: &[(&str, &str)]) -> TempDir {
    let mut all = vec![("Cargo.toml", consts::PROBE_MANIFEST), ("src/lib.rs", "")];
    all.extend_from_slice(files);
    git_checkout(&all)
}

/// The pool of lane `test` under the fleet root `lanes`.
pub(super) fn pool(lanes: &Path, freshness: LaneFreshness) -> SlotPool {
    SlotPool::fleet(lanes, CacheTrust::Review, "test", freshness)
}

/// The slot a claim of [`pool`] takes when no other job holds one.
pub(super) fn slot(lanes: &Path, freshness: LaneFreshness) -> PathBuf {
    lanes.join(match freshness {
        LaneFreshness::Mtime => "review-lane-test-0",
        LaneFreshness::Checksum => "review-ordered-lane-test-0",
    })
}

/// Leaves a build-script run at `key` (`<profile>/build/<run>`) whose
/// `output` holds `output`, under a profile cargo would recognise, and
/// returns that `output` file.
pub(super) fn write_unit(lane: &Path, key: &str, output: &str) -> PathBuf {
    let (profile, _) = key.split_once("/build/").unwrap();
    fs::create_dir_all(lane.join(profile).join(".fingerprint")).unwrap();
    let run = lane.join(key);
    fs::create_dir_all(&run).unwrap();
    let file = run.join("output");
    fs::write(&file, output).unwrap();
    file
}
