//! Cargo never removes a build unit, so a slot a lane builds in keeps every
//! unit any of its builds produced. A unit's use is the newest mtime in its
//! `.fingerprint` directory: nightly Cargo touches it on each reuse under
//! `CARGO_UNSTABLE_MTIME_ON_USE`, stable Cargo only when it rebuilds. A unit
//! goes when it was last used a whole window before the slot's latest use,
//! counted from that use rather than from now, so a slot idle for a week keeps
//! its latest build. An idle slot as a whole is the host budget's to remove.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result};
use tracing::{info, warn};

use super::layout::{profiles, subdirectories};
use crate::consts;

/// A build unit and the time its builds last used it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UnitUse {
    pub(crate) profile: PathBuf,
    pub(crate) hash: String,
    pub(crate) used: SystemTime,
}

/// Removes the units the slot's builds stopped using, the scratch its tests
/// left and its old timing reports. Runs under the slot's lock.
pub(super) fn prune(dir: &Path, window: Duration) -> Result<()> {
    info!("pruning lane slot {}", dir.display());
    let started = Instant::now();
    let units = unit_uses(dir)?;
    let cutoff = units
        .iter()
        .map(|unit| unit.used)
        .max()
        .and_then(|latest| latest.checked_sub(window));
    let mut stale: BTreeMap<&Path, BTreeSet<&str>> = BTreeMap::new();
    for unit in units
        .iter()
        .filter(|unit| cutoff.is_some_and(|cutoff| unit.used < cutoff))
    {
        stale
            .entry(unit.profile.as_path())
            .or_default()
            .insert(unit.hash.as_str());
    }
    let removed = stale.values().map(BTreeSet::len).sum::<usize>();
    let mut bytes = scratch(dir)?;
    for (profile, hashes) in &stale {
        bytes = bytes.saturating_add(remove_units(profile, hashes)?);
    }
    if let Some(cutoff) = cutoff {
        bytes = bytes.saturating_add(old_timings(dir, cutoff)?);
    }
    info!(
        "pruned lane slot {}: {removed} units and {bytes} bytes removed, {} units kept, {:.1} s",
        dir.display(),
        units.len().saturating_sub(removed),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Every build unit in `dir`, in each of its profiles, with the time its builds
/// last used it.
pub(crate) fn unit_uses(dir: &Path) -> Result<Vec<UnitUse>> {
    let mut units = Vec::new();
    for profile in profiles(dir)? {
        units.extend(units_of(&profile)?);
    }
    Ok(units)
}

/// A profile's build units, each with the time its builds last used it. A name
/// in `.fingerprint` or `build` that is not `<name>-<hash>` is an error: it is
/// a toolchain laying units out in a way this pruning does not know.
fn units_of(profile: &Path) -> Result<Vec<UnitUse>> {
    for entry in entries(&profile.join("build"))? {
        unit_hash(&entry)?;
    }
    let mut units = Vec::new();
    for fingerprint in entries(&profile.join(".fingerprint"))? {
        let hash = unit_hash(&fingerprint)?.to_owned();
        let mut used = SystemTime::UNIX_EPOCH;
        for file in entries(&fingerprint)? {
            let modified = fs::metadata(&file)
                .and_then(|metadata| metadata.modified())
                .with_context(|| format!("reading when {} was last used", file.display()))?;
            used = used.max(modified);
        }
        units.push(UnitUse {
            profile: profile.to_path_buf(),
            hash,
            used,
        });
    }
    Ok(units)
}

fn unit_hash(path: &Path) -> Result<&str> {
    entry_hash(path).with_context(|| {
        format!(
            "unknown cargo layout at {}: expected <name>-<16 hex digits>; update the lane slot \
             pruning for this toolchain",
            path.display()
        )
    })
}

/// The unit hash in a name Cargo gave a unit's file: `<name>-<16 hex digits>`,
/// before any extension.
fn entry_hash(path: &Path) -> Option<&str> {
    let stem = path.file_name()?.to_str()?.split('.').next()?;
    let (_, hash) = stem.rsplit_once('-')?;
    (hash.len() == consts::UNIT_HASH_LEN && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then_some(hash)
}

/// The files and directories Cargo names after a unit in `profile`, each with
/// that unit's hash. What it names without one, such as `deps/<crate>.d` of a
/// cdylib or the `rmeta*` directories, belongs to no unit.
pub(crate) fn unit_paths(profile: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut paths = Vec::new();
    for kind in [".fingerprint", "build", "deps", "examples"] {
        for path in entries(&profile.join(kind))? {
            if let Some(hash) = entry_hash(&path).map(str::to_owned) {
                paths.push((hash, path));
            }
        }
    }
    Ok(paths)
}

/// Removes a profile's files of the given units.
pub(crate) fn remove_units(profile: &Path, hashes: &BTreeSet<&str>) -> Result<u64> {
    let mut bytes = 0_u64;
    for (hash, path) in unit_paths(profile)? {
        if hashes.contains(hash.as_str()) {
            bytes = bytes.saturating_add(remove(&path));
        }
    }
    Ok(bytes)
}

/// Removes the scratch space tests left (`CARGO_TARGET_TMPDIR`): no build reads
/// it back, and nothing else uses it while the slot is held.
fn scratch(dir: &Path) -> Result<u64> {
    let mut bytes = 0_u64;
    for root in roots(dir)? {
        let tmp = root.join("tmp");
        if tmp.is_dir() {
            bytes = bytes.saturating_add(remove(&tmp));
        }
    }
    Ok(bytes)
}

/// Removes the timing reports the slot's builds last wrote before the cutoff,
/// by the same rule as its units.
fn old_timings(dir: &Path, cutoff: SystemTime) -> Result<u64> {
    let mut bytes = 0_u64;
    for root in roots(dir)? {
        for report in entries(&root.join("cargo-timings"))? {
            let modified = fs::symlink_metadata(&report)
                .and_then(|metadata| metadata.modified())
                .with_context(|| format!("reading {}", report.display()))?;
            if modified < cutoff {
                bytes = bytes.saturating_add(remove(&report));
            }
        }
    }
    Ok(bytes)
}

/// The slot and each directory in it: Cargo writes `tmp` and `cargo-timings`
/// at the top of a target directory, and a lane may nest one target in another.
fn roots(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut roots = subdirectories(dir)?;
    roots.push(dir.to_path_buf());
    Ok(roots)
}

/// Removes a file or a directory tree and returns the bytes it held. What
/// cannot be removed stays and is logged: pruning only saves disk, and a job
/// never fails for it.
fn remove(path: &Path) -> u64 {
    removed(path).unwrap_or_else(|error| {
        warn!("{error:#}; it stays in the lane slot");
        0
    })
}

fn removed(path: &Path) -> Result<u64> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("reading {}", path.display()))?;
    if metadata.is_dir() {
        let bytes = size(path)?;
        fs::remove_dir_all(path).with_context(|| format!("removing {}", path.display()))?;
        return Ok(bytes);
    }
    fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    Ok(metadata.len())
}

fn size(dir: &Path) -> Result<u64> {
    let mut bytes = 0_u64;
    for path in entries(dir)? {
        let metadata =
            fs::symlink_metadata(&path).with_context(|| format!("reading {}", path.display()))?;
        let held = if metadata.is_dir() {
            size(&path)?
        } else {
            metadata.len()
        };
        bytes = bytes.saturating_add(held);
    }
    Ok(bytes)
}

/// The paths in a directory; none when it does not exist.
fn entries(dir: &Path) -> Result<Vec<PathBuf>> {
    let listing = match fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("listing {}", dir.display())),
    };
    let mut paths = Vec::new();
    for entry in listing {
        paths.push(
            entry
                .with_context(|| format!("listing {}", dir.display()))?
                .path(),
        );
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn age(path: &Path, time: SystemTime) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(time)
            .unwrap();
    }

    /// One build unit the way Cargo lays it out in `profile`, last used at
    /// `used`. Returns every file it wrote.
    fn unit(profile: &Path, package: &str, hash: &str, used: SystemTime) -> Vec<PathBuf> {
        let fingerprint = profile
            .join(".fingerprint")
            .join(format!("{package}-{hash}"));
        let files = vec![
            fingerprint.join(format!("lib-{package}")),
            fingerprint.join(format!("lib-{package}.json")),
            profile
                .join("build")
                .join(format!("{package}-{hash}"))
                .join("output"),
            profile
                .join("deps")
                .join(format!("lib{package}-{hash}.rlib")),
            profile.join("deps").join(format!("{package}-{hash}.d")),
            profile.join("examples").join(format!("{package}-{hash}")),
        ];
        for file in &files {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, package).unwrap();
        }
        age(&files[0], used);
        age(&files[1], used);
        files
    }

    #[test]
    fn a_unit_the_slot_stopped_using_goes_in_every_profile_form() {
        let slot = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let mut gone = Vec::new();
        let mut kept = Vec::new();
        for profile in [
            "debug",
            "ci-tests-flash-off/test-release",
            "aarch64-apple-darwin/debug",
        ] {
            let profile = slot.path().join(profile);
            gone.extend(unit(
                &profile,
                "old",
                "0123456789abcdef",
                now - 2 * consts::DAY,
            ));
            kept.extend(unit(&profile, "fresh", "fedcba9876543210", now));
            let uplifted = profile.join("libold.rlib");
            fs::write(&uplifted, "old").unwrap();
            kept.push(uplifted);
        }
        let scratch = [
            slot.path().join("tmp/test"),
            slot.path().join("ci-tests-flash-off/tmp/test"),
        ];
        for file in &scratch {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, "scratch").unwrap();
        }
        let timings = slot.path().join("cargo-timings");
        fs::create_dir_all(&timings).unwrap();
        let old_report = timings.join("cargo-timing-20260920T000000Z.html");
        fs::write(&old_report, "old").unwrap();
        age(&old_report, now - 2 * consts::DAY);
        let latest_report = timings.join("cargo-timing.html");
        fs::write(&latest_report, "latest").unwrap();

        prune(slot.path(), consts::DAY).unwrap();

        for path in gone.iter().chain(&scratch).chain([&old_report]) {
            assert!(!path.exists(), "{} outlived the window", path.display());
        }
        for path in kept.iter().chain([&latest_report]) {
            assert!(path.exists(), "{} was removed while in use", path.display());
        }
    }

    /// The window counts back from the slot's latest use, not from now: a slot
    /// the lane left for a week still holds the build it will reuse.
    #[test]
    fn a_slot_idle_for_a_week_keeps_its_latest_build() {
        let slot = tempfile::tempdir().unwrap();
        let profile = slot.path().join("debug");
        let latest = SystemTime::now() - 7 * consts::DAY;
        let last = unit(&profile, "last", "0123456789abcdef", latest);
        let earlier = unit(
            &profile,
            "earlier",
            "1123456789abcdef",
            latest - consts::DAY / 2,
        );
        let gone = unit(
            &profile,
            "gone",
            "2123456789abcdef",
            latest - 2 * consts::DAY,
        );

        prune(slot.path(), consts::DAY).unwrap();

        assert!(
            last.iter().chain(&earlier).all(|path| path.exists()),
            "a week idle is no reason to drop the slot's latest build"
        );
        assert!(
            gone.iter().all(|path| !path.exists()),
            "a unit unused for the window before the latest build stayed"
        );
    }

    /// The next nightly lays build scripts out as `build/<pkg>/<META>/`. Pruning
    /// by a layout it does not know would remove the wrong files or none.
    #[test]
    fn a_unit_laid_out_another_way_stops_the_pruning_by_name() {
        let slot = tempfile::tempdir().unwrap();
        let profile = slot.path().join("debug");
        let now = SystemTime::now();
        let old = unit(&profile, "old", "0123456789abcdef", now - 2 * consts::DAY);
        unit(&profile, "fresh", "fedcba9876543210", now);
        let unknown = profile.join("build").join("serde");
        fs::create_dir_all(unknown.join("0123456789abcdef")).unwrap();

        let error = prune(slot.path(), consts::DAY).unwrap_err();

        assert!(
            format!("{error:#}").contains(&unknown.display().to_string()),
            "the error must name what it could not read: {error:#}"
        );
        assert!(
            old.iter().all(|path| path.exists()),
            "pruning went on past a layout it does not know"
        );
    }

    #[test]
    fn what_cargo_names_without_a_hash_is_left_alone() {
        let slot = tempfile::tempdir().unwrap();
        let profile = slot.path().join("debug");
        let now = SystemTime::now();
        unit(&profile, "gone", "0123456789abcdef", now - 2 * consts::DAY);
        unit(&profile, "fresh", "fedcba9876543210", now);
        let dep_info = profile.join("deps/kithara_ffi.d");
        let rmeta = profile.join("deps/rmetaAbC123/lib.rmeta");
        for file in [&dep_info, &rmeta] {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, "unhashed").unwrap();
        }

        prune(slot.path(), consts::DAY).unwrap();

        assert!(
            !profile.join("deps/libgone-0123456789abcdef.rlib").exists(),
            "the stale unit stayed, so this proves nothing"
        );
        assert!(dep_info.exists(), "a name without a hash is no unit's");
        assert!(rmeta.exists(), "a name without a hash is no unit's");
    }

    /// Pruning only saves disk: what it cannot remove stays, and the job that
    /// claimed the slot builds on.
    #[cfg(unix)]
    #[test]
    fn what_pruning_cannot_remove_stays_and_the_claim_goes_on() {
        use std::os::unix::fs::PermissionsExt as _;

        let slot = tempfile::tempdir().unwrap();
        let profile = slot.path().join("debug");
        let now = SystemTime::now();
        let gone = unit(&profile, "gone", "0123456789abcdef", now - 2 * consts::DAY);
        unit(&profile, "fresh", "fedcba9876543210", now);
        let sealed = slot.path().join("tmp/sealed");
        fs::create_dir_all(&sealed).unwrap();
        fs::write(sealed.join("left"), "scratch").unwrap();
        fs::set_permissions(&sealed, fs::Permissions::from_mode(0o555)).unwrap();

        let pruned = prune(slot.path(), consts::DAY);
        fs::set_permissions(&sealed, fs::Permissions::from_mode(0o755)).unwrap();

        pruned.expect("a path pruning cannot remove is no reason to fail the job");
        assert!(
            gone.iter().all(|path| !path.exists()),
            "pruning stopped at what it could not remove"
        );
    }
}
