//! A lane's build directory is shared by every checkout on the fleet, and
//! cargo judges freshness by mtime. A persistent checkout keeps the mtime of
//! every file a branch switch left alone, so artifacts another checkout built
//! from other sources can read as newer than those files and be reused
//! unbuilt. The directory records which content its artifacts may come from,
//! and a checkout that claims it stamps every file whose content is not the
//! only one recorded, so cargo rebuilds exactly those and reuses the rest.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

use anyhow::{Context, Result, bail};
use kithara_devtools::lock::FileLock;

struct Consts;

impl Consts {
    /// Held by the one job building in the directory.
    const LOCK_FILE: &str = ".kithara-lane.lock";
    /// The content the directory's artifacts may have been built from.
    const SOURCES_FILE: &str = ".kithara-lane-sources";
}

/// Git blob ids per tracked path.
type Sources = BTreeMap<String, BTreeSet<String>>;

/// One job's hold on a lane build directory.
pub(super) struct LaneBuild {
    _lock: FileLock,
    record: PathBuf,
    tracked: Sources,
}

impl LaneBuild {
    /// Waits out any other job building the same lane, then stamps the
    /// checkout's files the directory may hold artifacts of other content for.
    /// The record keeps that content too until the lane succeeds, so a job
    /// that dies mid-build leaves the next one stamping the same files.
    pub(super) fn claim(project_root: &Path, dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir)
            .with_context(|| format!("creating lane build directory {}", dir.display()))?;
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(Consts::LOCK_FILE))
            .with_context(|| format!("opening the lane build lock in {}", dir.display()))?;
        let lock = FileLock::exclusive(lock).context("waiting for the lane build lock")?;
        let tracked = tracked_sources(project_root)?;
        let record = dir.join(Consts::SOURCES_FILE);
        let mut recorded = read_sources(&record)?;
        let now = SystemTime::now();
        for path in stale_paths(&recorded, &tracked) {
            let file = project_root.join(path);
            File::options()
                .write(true)
                .open(&file)
                .and_then(|file| file.set_modified(now))
                .with_context(|| format!("stamping {}", file.display()))?;
        }
        for (path, blobs) in &tracked {
            recorded
                .entry(path.clone())
                .or_default()
                .extend(blobs.iter().cloned());
        }
        write_sources(&record, &recorded)?;
        Ok(Self {
            _lock: lock,
            record,
            tracked,
        })
    }

    /// A lane that succeeded built what it reads from this checkout's content.
    pub(super) fn settle(&self) -> Result<()> {
        write_sources(&self.record, &self.tracked)
    }
}

/// Paths whose recorded content is anything but exactly what is checked out.
fn stale_paths<'a>(recorded: &Sources, tracked: &'a Sources) -> Vec<&'a str> {
    tracked
        .iter()
        .filter(|(path, blobs)| recorded.get(*path) != Some(*blobs))
        .map(|(path, _)| path.as_str())
        .collect()
}

fn tracked_sources(project_root: &Path) -> Result<Sources> {
    let output = Command::new("git")
        .current_dir(project_root)
        .args(["ls-files", "--stage", "-z"])
        .output()
        .context("listing the checkout's tracked files")?;
    if !output.status.success() {
        bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(parse_stage(&String::from_utf8_lossy(&output.stdout)))
}

/// `git ls-files --stage -z` entries are `<mode> <blob> <stage>\t<path>`.
/// Symlinks and submodules carry no content cargo reads through them.
fn parse_stage(listed: &str) -> Sources {
    let mut sources = Sources::new();
    for entry in listed.split('\0') {
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        let mut meta = meta.split(' ');
        let (Some(mode), Some(blob)) = (meta.next(), meta.next()) else {
            continue;
        };
        if mode == "120000" || mode == "160000" || path.contains('\n') {
            continue;
        }
        sources
            .entry(path.to_owned())
            .or_default()
            .insert(blob.to_owned());
    }
    sources
}

fn read_sources(record: &Path) -> Result<Sources> {
    let text = match fs::read_to_string(record) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Sources::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", record.display()));
        }
    };
    let mut sources = Sources::new();
    for line in text.lines() {
        if let Some((blob, path)) = line.split_once('\t') {
            sources
                .entry(path.to_owned())
                .or_default()
                .insert(blob.to_owned());
        }
    }
    Ok(sources)
}

fn write_sources(record: &Path, sources: &Sources) -> Result<()> {
    let mut text = String::new();
    for (path, blobs) in sources {
        for blob in blobs {
            text.push_str(blob);
            text.push('\t');
            text.push_str(path);
            text.push('\n');
        }
    }
    let partial = record.with_extension("partial");
    fs::write(&partial, text).with_context(|| format!("writing {}", partial.display()))?;
    fs::rename(&partial, record).with_context(|| format!("replacing {}", record.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(entries: &[(&str, &str)]) -> Sources {
        let mut sources = Sources::new();
        for (path, blob) in entries {
            sources
                .entry((*path).to_owned())
                .or_default()
                .insert((*blob).to_owned());
        }
        sources
    }

    #[test]
    fn only_content_the_directory_did_not_build_alone_is_stamped() {
        let recorded = sources(&[
            ("same.rs", "a"),
            ("changed.rs", "b"),
            ("mixed.rs", "c"),
            ("mixed.rs", "d"),
        ]);
        let tracked = sources(&[
            ("same.rs", "a"),
            ("changed.rs", "e"),
            ("mixed.rs", "c"),
            ("new.rs", "f"),
        ]);

        assert_eq!(
            stale_paths(&recorded, &tracked),
            ["changed.rs", "mixed.rs", "new.rs"]
        );
    }

    #[test]
    fn stage_listing_skips_links_and_submodules() {
        let listed = "100644 aaa 0\tsrc/lib.rs\0120000 bbb 0\tlink\0160000 ccc 0\tvendor\0";

        assert_eq!(parse_stage(listed), sources(&[("src/lib.rs", "aaa")]));
    }

    /// The branch that built the lane last must not hand a file's artifacts to
    /// a checkout whose unchanged copy of that file is older than the build.
    #[test]
    fn a_claim_stamps_what_another_branch_built_until_the_lane_succeeds() {
        let checkout = tempfile::tempdir().unwrap();
        let lane = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .current_dir(checkout.path())
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        fs::write(checkout.path().join("lib.rs"), "one").unwrap();
        git(&["add", "lib.rs"]);
        let file = checkout.path().join("lib.rs");
        let old = SystemTime::UNIX_EPOCH;
        File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(old)
            .unwrap();
        write_sources(
            &lane.path().join(Consts::SOURCES_FILE),
            &sources(&[("lib.rs", "other")]),
        )
        .unwrap();

        let claim = LaneBuild::claim(checkout.path(), lane.path()).unwrap();

        assert!(
            fs::metadata(&file).unwrap().modified().unwrap() > old,
            "stamped"
        );
        claim.settle().unwrap();
        drop(claim);
        File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let _claim = LaneBuild::claim(checkout.path(), lane.path()).unwrap();
        assert_eq!(
            fs::metadata(&file).unwrap().modified().unwrap(),
            old,
            "a settled lane reuses what it built from this content"
        );
    }
}
