//! An mtime lane's slot records which content its artifacts may come from,
//! and a checkout that claims it stamps every file whose content is not the
//! only one recorded, so cargo rebuilds exactly those and reuses the rest. A
//! build the record did not see leaves artifacts of unknown content, so every
//! file is stamped until a lane succeeds again.

use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result};

use super::{
    layout::{profiles, subdirectories},
    tracked::Sources,
};
use crate::consts;

/// What an mtime lane's claim recorded, for its settle.
pub(super) struct Claimed {
    record: PathBuf,
    claimed: Sources,
    tracked: Sources,
}

/// Stamps the checkout's files the slot may hold artifacts of other content
/// for. The record keeps that content too until the lane succeeds, so a job
/// that dies mid-build leaves the next one stamping the same files.
pub(super) fn claim(project_root: &Path, dir: &Path, tracked: Sources) -> Result<Claimed> {
    let record = dir.join(consts::SOURCES_FILE);
    let mut recorded = read_sources(&record)?;
    if unseen_build(dir, &record)? {
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
    Ok(Claimed {
        record,
        claimed: recorded,
        tracked,
    })
}

impl Claimed {
    /// Records the job's builds as seen. A failed lane invalidates every
    /// tracked source because its cached artifacts did not prove trustworthy.
    pub(super) fn settle(&self, succeeded: bool) -> Result<()> {
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
    use crate::{
        ci::{
            environment::CacheTrust,
            lane_build::{
                LaneBuild, SlotPool,
                fixture::{git_checkout, mtime, set_mtime, sources},
                tracked,
            },
        },
        config::LaneFreshness,
    };

    /// A git checkout holding `lib.rs`, whose mtime is the epoch.
    fn lib_checkout() -> (tempfile::TempDir, PathBuf) {
        let checkout = git_checkout(&[("lib.rs", "one")]);
        let file = checkout.path().join("lib.rs");
        set_mtime(&file, SystemTime::UNIX_EPOCH);
        (checkout, file)
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
    fn a_claim_stamps_what_another_branch_built_until_the_lane_succeeds() {
        let (checkout, file) = lib_checkout();
        let lanes = tempfile::tempdir().unwrap();
        let pool = SlotPool::fleet(lanes.path(), CacheTrust::Review, "test");
        let slot = lanes.path().join("review-lane-test-0");
        fs::create_dir_all(&slot).unwrap();
        write_sources(
            &slot.join(consts::SOURCES_FILE),
            &sources(&[("lib.rs", "other")]),
        )
        .unwrap();

        let claim =
            LaneBuild::claim(checkout.path(), &pool, consts::DAY, LaneFreshness::Mtime).unwrap();

        assert_eq!(claim.dir(), slot.as_path());
        assert!(mtime(&file) > SystemTime::UNIX_EPOCH, "stamped");
        claim.settle(true).unwrap();
        drop(claim);
        set_mtime(&file, SystemTime::UNIX_EPOCH);
        let _claim =
            LaneBuild::claim(checkout.path(), &pool, consts::DAY, LaneFreshness::Mtime).unwrap();
        assert_eq!(
            mtime(&file),
            SystemTime::UNIX_EPOCH,
            "a settled lane reuses what it built from this content"
        );
    }

    #[test]
    fn a_build_the_record_did_not_see_stamps_everything_until_the_lane_succeeds() {
        let (checkout, file) = lib_checkout();
        let lanes = tempfile::tempdir().unwrap();
        let pool = SlotPool::fleet(lanes.path(), CacheTrust::Review, "test");
        let slot = lanes.path().join("review-lane-test-0");
        fs::create_dir_all(&slot).unwrap();
        let record = slot.join(consts::SOURCES_FILE);
        write_sources(&record, &tracked::list(checkout.path()).unwrap()).unwrap();
        set_mtime(&record, SystemTime::UNIX_EPOCH);
        let unit = slot.join("debug/.fingerprint/lib-0123456789abcdef");
        fs::create_dir_all(&unit).unwrap();
        fs::write(unit.join("lib-lib"), "hash").unwrap();

        let claim =
            LaneBuild::claim(checkout.path(), &pool, consts::DAY, LaneFreshness::Mtime).unwrap();
        assert!(mtime(&file) > SystemTime::UNIX_EPOCH, "stamped");
        claim.settle(false).unwrap();
        drop(claim);

        set_mtime(&file, SystemTime::UNIX_EPOCH);
        let claim =
            LaneBuild::claim(checkout.path(), &pool, consts::DAY, LaneFreshness::Mtime).unwrap();
        assert!(
            mtime(&file) > SystemTime::UNIX_EPOCH,
            "a failed lane leaves the unseen content recorded"
        );
        claim.settle(true).unwrap();
    }

    #[test]
    fn a_failed_lane_invalidates_every_tracked_source() {
        let (checkout, file) = lib_checkout();
        let lanes = tempfile::tempdir().unwrap();
        let pool = SlotPool::fleet(lanes.path(), CacheTrust::Review, "test");

        let claim =
            LaneBuild::claim(checkout.path(), &pool, consts::DAY, LaneFreshness::Mtime).unwrap();
        claim.settle(false).unwrap();
        drop(claim);
        set_mtime(&file, SystemTime::UNIX_EPOCH);

        let _claim =
            LaneBuild::claim(checkout.path(), &pool, consts::DAY, LaneFreshness::Mtime).unwrap();
        assert!(
            mtime(&file) > SystemTime::UNIX_EPOCH,
            "a failed lane cannot certify cached artifacts"
        );
    }
}
