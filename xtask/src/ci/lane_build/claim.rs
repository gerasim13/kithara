use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, bail};
use kithara_devtools::lock::FileLock;

use super::{
    layout::{profiles, subdirectories},
    pool::SlotPool,
    prune,
};
use crate::consts;

/// Git blob ids per tracked path.
type Sources = BTreeMap<String, BTreeSet<String>>;

/// One job's hold on a lane slot.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get, deref = false)]
pub(crate) struct LaneBuild {
    _lock: FileLock,
    #[field(
        deref = Path,
        get(vis = "pub(crate)", doc = "The directory Cargo builds in.")
    )]
    dir: PathBuf,
    record: PathBuf,
    claimed: Sources,
    tracked: Sources,
}

impl LaneBuild {
    /// Takes the first free slot of the pool and prunes the units its builds
    /// stopped using, then stamps the checkout's files the slot may hold
    /// artifacts of other content for. The record keeps that content too until
    /// the lane succeeds, so a job that dies mid-build leaves the next one
    /// stamping the same files.
    pub(crate) fn claim(project_root: &Path, pool: &SlotPool, window: Duration) -> Result<Self> {
        let (dir, lock) = pool.take()?;
        fs::create_dir_all(&dir)
            .with_context(|| format!("creating lane build directory {}", dir.display()))?;
        prune::prune(&dir, window)?;
        let tracked = tracked_sources(project_root)?;
        let record = dir.join(consts::SOURCES_FILE);
        let mut recorded = read_sources(&record)?;
        if unseen_build(&dir, &record)? {
            for path in tracked.keys() {
                recorded
                    .entry(path.clone())
                    .or_default()
                    .insert(consts::UNKNOWN_BLOB.to_owned());
            }
        }
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
            dir,
            record,
            claimed: recorded,
            tracked,
        })
    }

    /// Records the job's builds as seen. A failed lane invalidates every
    /// tracked source because its cached artifacts did not prove trustworthy.
    pub(crate) fn settle(&self, succeeded: bool) -> Result<()> {
        if succeeded {
            return write_sources(&self.record, &self.tracked);
        }
        let mut uncertain = self.claimed.clone();
        for path in self.tracked.keys() {
            uncertain
                .entry(path.clone())
                .or_default()
                .insert(consts::UNKNOWN_BLOB.to_owned());
        }
        write_sources(&self.record, &uncertain)
    }
}

/// Whether cargo wrote a unit fingerprint after the record was last written.
fn unseen_build(dir: &Path, record: &Path) -> Result<bool> {
    let seen = match fs::metadata(record) {
        Ok(metadata) => metadata.modified()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", record.display()));
        }
    };
    for profile in profiles(dir)? {
        for unit in subdirectories(&profile.join(".fingerprint"))? {
            for file in
                fs::read_dir(&unit).with_context(|| format!("listing {}", unit.display()))?
            {
                if file?.metadata()?.modified()? > seen {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
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
    use crate::ci::environment::CacheTrust;

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

    /// A git checkout holding `lib.rs`, whose mtime is the epoch. Ambient
    /// `GIT_*` variables of a hook would point git at the repository running
    /// the test and have `git add` write its index.
    fn git_checkout() -> (tempfile::TempDir, PathBuf) {
        let checkout = tempfile::tempdir().unwrap();
        let file = checkout.path().join("lib.rs");
        fs::write(&file, "one").unwrap();
        for args in [["init", "-q"], ["add", "lib.rs"]] {
            let status = Command::new("git")
                .current_dir(checkout.path())
                .args(args)
                .env_remove("GIT_DIR")
                .env_remove("GIT_INDEX_FILE")
                .env_remove("GIT_WORK_TREE")
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        }
        set_old(&file);
        (checkout, file)
    }

    fn set_old(path: &Path) {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH)
            .unwrap();
    }

    fn modified(path: &Path) -> SystemTime {
        fs::metadata(path).unwrap().modified().unwrap()
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

    /// The branch that built the slot last must not hand a file's artifacts to
    /// a checkout whose unchanged copy of that file is older than the build.
    #[test]
    fn a_claim_stamps_what_another_branch_built_until_the_lane_succeeds() {
        let (checkout, file) = git_checkout();
        let lanes = tempfile::tempdir().unwrap();
        let pool = SlotPool::fleet(lanes.path(), CacheTrust::Review, "test");
        let slot = lanes.path().join("review-lane-test-0");
        fs::create_dir_all(&slot).unwrap();
        write_sources(
            &slot.join(consts::SOURCES_FILE),
            &sources(&[("lib.rs", "other")]),
        )
        .unwrap();

        let claim = LaneBuild::claim(checkout.path(), &pool, consts::DAY).unwrap();

        assert_eq!(claim.dir(), slot.as_path());
        assert!(modified(&file) > SystemTime::UNIX_EPOCH, "stamped");
        claim.settle(true).unwrap();
        drop(claim);
        set_old(&file);
        let _claim = LaneBuild::claim(checkout.path(), &pool, consts::DAY).unwrap();
        assert_eq!(
            modified(&file),
            SystemTime::UNIX_EPOCH,
            "a settled lane reuses what it built from this content"
        );
    }

    /// A job without the claim may have rebuilt any unit from other content.
    #[test]
    fn a_build_the_record_did_not_see_stamps_everything_until_the_lane_succeeds() {
        let (checkout, file) = git_checkout();
        let lanes = tempfile::tempdir().unwrap();
        let pool = SlotPool::fleet(lanes.path(), CacheTrust::Review, "test");
        let slot = lanes.path().join("review-lane-test-0");
        fs::create_dir_all(&slot).unwrap();
        let record = slot.join(consts::SOURCES_FILE);
        write_sources(&record, &tracked_sources(checkout.path()).unwrap()).unwrap();
        set_old(&record);
        let unit = slot.join("debug/.fingerprint/lib-0123456789abcdef");
        fs::create_dir_all(&unit).unwrap();
        fs::write(unit.join("lib-lib"), "hash").unwrap();

        let claim = LaneBuild::claim(checkout.path(), &pool, consts::DAY).unwrap();
        assert!(modified(&file) > SystemTime::UNIX_EPOCH, "stamped");
        claim.settle(false).unwrap();
        drop(claim);

        set_old(&file);
        let claim = LaneBuild::claim(checkout.path(), &pool, consts::DAY).unwrap();
        assert!(
            modified(&file) > SystemTime::UNIX_EPOCH,
            "a failed lane leaves the unseen content recorded"
        );
        claim.settle(true).unwrap();
    }

    #[test]
    fn a_failed_lane_invalidates_every_tracked_source() {
        let (checkout, file) = git_checkout();
        let lanes = tempfile::tempdir().unwrap();
        let pool = SlotPool::fleet(lanes.path(), CacheTrust::Review, "test");

        let claim = LaneBuild::claim(checkout.path(), &pool, consts::DAY).unwrap();
        claim.settle(false).unwrap();
        drop(claim);
        set_old(&file);

        let _claim = LaneBuild::claim(checkout.path(), &pool, consts::DAY).unwrap();
        assert!(
            modified(&file) > SystemTime::UNIX_EPOCH,
            "a failed lane cannot certify cached artifacts"
        );
    }

    /// Cargo never removes a unit, so a slot every branch builds in grows by
    /// each branch's units until the claim removes what nothing uses.
    #[test]
    fn a_claim_prunes_what_the_lane_stopped_using() {
        let (checkout, _file) = git_checkout();
        let lanes = tempfile::tempdir().unwrap();
        let pool = SlotPool::fleet(lanes.path(), CacheTrust::Review, "test");
        let fingerprints = lanes.path().join("review-lane-test-0/debug/.fingerprint");
        let old = fingerprints.join("old-0123456789abcdef");
        let fresh = fingerprints.join("fresh-fedcba9876543210");
        for unit in [&old, &fresh] {
            fs::create_dir_all(unit).unwrap();
            fs::write(unit.join("lib"), "hash").unwrap();
        }
        File::options()
            .write(true)
            .open(old.join("lib"))
            .unwrap()
            .set_modified(SystemTime::now() - 2 * consts::DAY)
            .unwrap();

        let _claim = LaneBuild::claim(checkout.path(), &pool, consts::DAY).unwrap();

        assert!(
            !old.exists(),
            "a unit the lane stopped using outlived the claim"
        );
        assert!(
            fresh.exists(),
            "the claim removed a unit the lane still uses"
        );
    }
}
