#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
    process,
    time::SystemTime,
};

use anyhow::{Context, Result, bail};
use fs4::TryLockError;
use kithara_devtools::{lease, lock::FileLock};
use tracing::info;

use crate::consts;

/// One build directory in a build root: a lane's, a stress run's, xtask's own.
#[derive(Clone, Debug, Eq, PartialEq)]
struct CacheEntry {
    path: PathBuf,
    size_bytes: u64,
    /// When a job last took the entry's lease. A directory no job ever leased
    /// is left from a layout no lane builds in any more, and dates from the
    /// epoch, before every directory a lane has used.
    used: SystemTime,
}

struct CacheContents {
    entries: Vec<CacheEntry>,
    /// Entries a live job holds. They are charged against the ceiling and
    /// never evicted.
    held: Vec<CacheEntry>,
}

/// The least recently used entries, oldest first, until what is left fits the
/// budget.
fn select_evictions(mut entries: Vec<CacheEntry>, budget_bytes: u64) -> Vec<CacheEntry> {
    let mut remaining_bytes = total_bytes(&entries);
    entries.sort_by(|left, right| {
        left.used
            .cmp(&right.used)
            .then_with(|| left.path.cmp(&right.path))
    });
    entries
        .into_iter()
        .take_while(|entry| {
            let over = remaining_bytes > budget_bytes;
            remaining_bytes = remaining_bytes.saturating_sub(entry.size_bytes);
            over
        })
        .collect()
}

/// The budget is what the host can afford in total, not what one checkout may
/// keep.
///
/// Applied per directory it never fires on a machine that is running out:
/// three checkouts holding 14, 7 and 22 GB were each under a 25 GB budget, so
/// every hourly pass reported `bytes_freed=0` while the volume they share sat
/// at `Aggressive` and jobs were already being refused. A directory an active
/// job holds cannot be evicted, but the room it occupies is still spent, so it
/// is charged against the ceiling rather than excused from it.
pub(crate) fn enforce_budget(target_dirs: &[PathBuf], budget_bytes: u64) -> Result<()> {
    let (candidates, held_bytes) = collect(target_dirs)?;
    evict_to_budget(candidates, held_bytes, budget_bytes)
}

/// Evict until at least `bytes_needed` is gone, whatever the ceiling says.
///
/// A job that cannot fit asks a different question than the hourly pass does.
/// The ceiling answers "may the host keep this much"; caches under it are left
/// alone, which is right for a scheduled sweep and useless for a job standing
/// in front of a full volume — 15.7 GB of caches under a 25 GB ceiling freed
/// nothing while the job needed one more gigabyte and was refused. Here the
/// shortfall is the budget: the oldest entries go until it is covered, or until
/// nothing evictable is left and the caller reports the volume as it is.
pub(crate) fn reclaim_at_least(target_dirs: &[PathBuf], bytes_needed: u64) -> Result<()> {
    let (candidates, held_bytes) = collect(target_dirs)?;
    let keep = total_bytes(&candidates).saturating_sub(bytes_needed);
    evict_to_budget(candidates, held_bytes, keep.saturating_add(held_bytes))
}

fn collect(target_dirs: &[PathBuf]) -> Result<(Vec<CacheEntry>, u64)> {
    let mut target_dirs = target_dirs.to_vec();
    target_dirs.sort();
    let mut candidates = Vec::new();
    let mut held_bytes = 0_u64;
    for target_dir in &target_dirs {
        let contents = candidate_entries(target_dir)?;
        let held = total_bytes(&contents.held);
        held_bytes = held_bytes.saturating_add(held);
        if held > 0 {
            info!(
                path = %target_dir.display(),
                held_bytes = held,
                "keeping the build directories live jobs hold"
            );
        }
        candidates.extend(contents.entries);
    }
    Ok((candidates, held_bytes))
}

fn total_bytes(entries: &[CacheEntry]) -> u64 {
    entries
        .iter()
        .map(|entry| entry.size_bytes)
        .fold(0_u64, u64::saturating_add)
}

fn evict_to_budget(candidates: Vec<CacheEntry>, held_bytes: u64, budget_bytes: u64) -> Result<()> {
    let bytes_before = total_bytes(&candidates).saturating_add(held_bytes);
    let mut bytes_freed = 0_u64;
    for entry in select_evictions(candidates, budget_bytes.saturating_sub(held_bytes)) {
        bytes_freed = bytes_freed.saturating_add(evict(&entry)?);
    }
    info!(
        bytes_before,
        bytes_freed, held_bytes, budget_bytes, "build cache budget enforced"
    );
    Ok(())
}

/// Removes one entry and returns the bytes it held, or nothing when a job took
/// it after the scan.
///
/// The entry is moved aside under the eviction's exclusive lease before
/// anything in it is removed, so a job that enters it meanwhile finds its path
/// free and builds in a new directory instead of one being emptied under it.
fn evict(entry: &CacheEntry) -> Result<u64> {
    let _eviction = match lease::evict(&entry.path) {
        Ok(Some(eviction)) => eviction,
        Ok(None) => {
            info!(
                path = %entry.path.display(),
                "keeping the build directory a job took after the scan"
            );
            return Ok(0);
        }
        // Another eviction moved it aside first.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("fencing build directory {}", entry.path.display()));
        }
    };
    if heartbeat_is_fresh(&entry.path.join(lease::HEARTBEAT)) {
        info!(
            path = %entry.path.display(),
            "keeping the build directory a job in a virtual machine took after the scan"
        );
        return Ok(0);
    }
    let aside = aside(&entry.path)?;
    fs::rename(&entry.path, &aside).with_context(|| {
        format!(
            "moving build directory {} aside to {}",
            entry.path.display(),
            aside.display()
        )
    })?;
    info!(path = %entry.path.display(), bytes = entry.size_bytes, "evicting build cache");
    remove_aside(&aside)?;
    Ok(entry.size_bytes)
}

/// Where an eviction moves `entry` before removing it: a hidden name in the
/// same root, which no build takes, unique to this eviction.
fn aside(entry: &Path) -> Result<PathBuf> {
    let (Some(root), Some(name)) = (entry.parent(), entry.file_name()) else {
        bail!("build directory {} names no root", entry.display());
    };
    let at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .context("reading the clock")?
        .as_nanos();
    let mut aside = OsString::from(consts::EVICTING_PREFIX);
    aside.push(name);
    aside.push(format!("-{}-{at}", process::id()));
    Ok(root.join(aside))
}

/// Removes a directory an eviction moved aside, its lease last: a pass that
/// finds the lease finds the eviction holding it, and one that finds no lease
/// finds only the empty directory left to remove.
fn remove_aside(aside: &Path) -> Result<()> {
    let listing =
        fs::read_dir(aside).with_context(|| format!("reading build cache {}", aside.display()))?;
    for child in listing {
        let child = child
            .with_context(|| format!("reading an entry in build cache {}", aside.display()))?;
        if child.file_name() == lease::FILE {
            continue;
        }
        let path = child.path();
        let removed = if child.file_type()?.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        removed.with_context(|| format!("removing build cache {}", path.display()))?;
    }
    still_there(fs::remove_file(aside.join(lease::FILE)))
        .with_context(|| format!("removing the lease of {}", aside.display()))?;
    still_there(fs::remove_dir(aside))
        .with_context(|| format!("removing build cache {}", aside.display()))?;
    Ok(())
}

/// Removes what an eviction that died midway moved aside, unless the eviction
/// is still running and holds its lease.
fn remove_leftover(aside: &Path) -> Result<()> {
    let lease_file = aside.join(lease::FILE);
    let file = match OpenOptions::new().read(true).write(true).open(&lease_file) {
        Ok(file) => file,
        // The lease goes last, so what is left is the empty directory, which
        // an eviction may be removing right now.
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            still_there(fs::remove_dir_all(aside))
                .with_context(|| format!("removing build cache {}", aside.display()))?;
            return Ok(());
        }
        Err(error) => {
            return Err(error).with_context(|| format!("opening {}", lease_file.display()));
        }
    };
    match FileLock::try_exclusive(file) {
        Ok(_eviction) => remove_aside(aside),
        Err(TryLockError::WouldBlock) => Ok(()),
        Err(TryLockError::Error(error)) => {
            Err(error).with_context(|| format!("locking {}", lease_file.display()))
        }
    }
}

/// What a listed path still is, or nothing when it is already gone.
///
/// The cache being measured is one a job may be building in, and a compiler
/// writes a temporary file and removes it again. A name the listing returned
/// and the build has since deleted is that race, not a broken cache: there is
/// nothing left to count or to reclaim. Any other failure is a real one, and
/// stopping the sweep on it is why the budget went unenforced.
fn still_there<T>(found: io::Result<T>) -> io::Result<Option<T>> {
    match found {
        Ok(found) => Ok(Some(found)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn candidate_entries(target_dir: &Path) -> Result<CacheContents> {
    if target_dir.parent().is_none() || !target_dir.is_absolute() {
        bail!(
            "refusing to inspect unsafe build cache path {}",
            target_dir.display()
        );
    }
    let metadata = fs::symlink_metadata(target_dir)
        .with_context(|| format!("reading build cache metadata for {}", target_dir.display()))?;
    if !metadata.file_type().is_dir() {
        bail!(
            "build cache path is not a directory: {}",
            target_dir.display()
        );
    }
    let listing = fs::read_dir(target_dir)
        .with_context(|| format!("reading build cache {}", target_dir.display()))?;
    let mut contents = CacheContents {
        entries: Vec::new(),
        held: Vec::new(),
    };
    for entry in listing {
        let entry = entry
            .with_context(|| format!("reading an entry in build cache {}", target_dir.display()))?;
        let path = entry.path();
        let Some(metadata) = still_there(fs::symlink_metadata(&path))
            .with_context(|| format!("reading build cache metadata for {}", path.display()))?
        else {
            continue;
        };
        // The build alias is a link to an entry listed beside it.
        if !metadata.file_type().is_dir() {
            continue;
        }
        if is_leftover(&path) {
            remove_leftover(&path)?;
            continue;
        }
        if is_hidden(&path) {
            continue;
        }
        let lease_file = path.join(lease::FILE);
        let used = match still_there(fs::metadata(&lease_file))
            .with_context(|| format!("reading {}", lease_file.display()))?
        {
            Some(lease) => lease
                .modified()
                .with_context(|| format!("reading the date of {}", lease_file.display()))?,
            None => SystemTime::UNIX_EPOCH,
        };
        let held =
            lease_file_is_held(&lease_file) || heartbeat_is_fresh(&path.join(lease::HEARTBEAT));
        let entry = CacheEntry {
            size_bytes: directory_bytes(&path)?,
            used,
            path,
        };
        if held {
            contents.held.push(entry);
        } else {
            contents.entries.push(entry);
        }
    }
    Ok(contents)
}

/// Cargo writes no hidden directory at the top of a build directory, so one
/// there belongs to something else. On the Linux fleet it is the CI cache root,
/// Cargo home included, which the per-runner build directory carries: evicted
/// as the oldest entry, it took every running job's registry sources and git
/// checkouts with it on each two-hourly pass.
fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.as_encoded_bytes().starts_with(b"."))
}

fn is_leftover(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        name.as_encoded_bytes()
            .starts_with(consts::EVICTING_PREFIX.as_bytes())
    })
}

fn directory_bytes(path: &Path) -> Result<u64> {
    let Some(metadata) = still_there(fs::symlink_metadata(path))
        .with_context(|| format!("reading build cache metadata for {}", path.display()))?
    else {
        return Ok(0);
    };
    let mut bytes = allocated_bytes(&metadata);
    if !metadata.file_type().is_dir() {
        return Ok(bytes);
    }
    let Some(listing) = still_there(fs::read_dir(path))
        .with_context(|| format!("reading build cache directory {}", path.display()))?
    else {
        return Ok(bytes);
    };
    for entry in listing {
        let entry =
            entry.with_context(|| format!("reading an entry in build cache {}", path.display()))?;
        bytes = bytes.saturating_add(directory_bytes(&entry.path())?);
    }
    Ok(bytes)
}

#[cfg(unix)]
fn allocated_bytes(metadata: &fs::Metadata) -> u64 {
    metadata.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn allocated_bytes(metadata: &fs::Metadata) -> u64 {
    metadata.len()
}

/// Whether a live job holds the lease at `path`.
///
/// `.cargo-lock` cannot answer that: Cargo holds it only while it compiles, so
/// a directory whose tests are already running looks abandoned. A reclaim that
/// believed it deleted a live `target` out from under a sibling job, and the
/// 1869 tests that then failed to exec their own binaries looked like the
/// product breaking rather than the CI eating itself. Holders take the lease
/// shared so several coexist, so only an exclusive request sees them. The
/// lease is only opened, never made: a directory the scan merely looked at is
/// not one a job leased.
fn lease_file_is_held(path: &Path) -> bool {
    let Ok(file) = OpenOptions::new().read(true).write(true).open(path) else {
        return false;
    };
    // Held by a live job, or unreadable — either way, not ours to remove.
    FileLock::try_exclusive(file).is_err()
}

fn heartbeat_is_fresh(path: &Path) -> bool {
    let Ok(modified) = fs::metadata(path).and_then(|metadata| metadata.modified()) else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .map_or(true, |age| age <= consts::HEARTBEAT_MAX_AGE)
}

/// Claims `CARGO_TARGET_DIR` for the life of this process, if one is named.
///
/// The checkout lease cannot protect it: on Linux runners the target is a
/// per-runner Docker volume the host budgets directly, and a lease on the
/// checkout says nothing about a directory outside it. A lane that builds into
/// a directory of its own claims that one where it names it, so both claims are
/// the one protocol [`lease`] owns and [`lease_file_is_held`] asks about.
pub(crate) fn hold_target_lease() -> Result<Option<lease::Lease>> {
    lease_target(env::var_os("CARGO_TARGET_DIR").map(PathBuf::from))
}

/// A build alias is skipped: it is a link `ci lane` points at the lane's own
/// directory, and leases that directory itself once it knows the lane.
fn lease_target(target: Option<PathBuf>) -> Result<Option<lease::Lease>> {
    target
        .filter(|target| target.file_name() != Some(OsStr::new(consts::BUILD_ALIAS)))
        .map(|target| lease::hold(&target).with_context(|| format!("lease {}", target.display())))
        .transpose()
}

/// Every build root under `root`, so a caller can hand them to
/// [`enforce_budget`]: a directory holding the build alias or xtask's own
/// build, as the one an executor names beside each checkout does. Shared with
/// the environment gate: refusing a job is only honest once these have been
/// reclaimed.
///
/// The walk stops at a checkout, whose tree is the job's and holds no build,
/// and at a root, whose entries are the builds the budget weighs. A root a job
/// is building in is listed like any other: the lease on the build it entered
/// keeps that one, and its bytes are charged against the ceiling, which is
/// what leaves the idle builds beside it payable.
pub(crate) fn build_roots(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }

    let mut pending = vec![root.to_path_buf()];
    let mut roots = Vec::new();
    while let Some(directory) = pending.pop() {
        if directory.join("Cargo.toml").is_file() {
            continue;
        }
        if [consts::BUILD_ALIAS, consts::XTASK_BUILD]
            .iter()
            .any(|name| fs::symlink_metadata(directory.join(name)).is_ok())
        {
            roots.push(directory);
            continue;
        }
        let entries = fs::read_dir(&directory)
            .with_context(|| format!("reading CI workspace directory {}", directory.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| {
                format!(
                    "reading an entry in CI workspace directory {}",
                    directory.display()
                )
            })?;
            if is_hidden(&entry.path()) {
                continue;
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .with_context(|| format!("reading CI workspace metadata for {}", path.display()))?;
            if metadata.file_type().is_dir() {
                pending.push(path);
            }
        }
    }
    roots.sort();
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{File, FileTimes},
        time::{Duration, UNIX_EPOCH},
    };

    use super::*;

    fn entry(path: &str, size_bytes: u64, age: u64) -> CacheEntry {
        CacheEntry {
            path: PathBuf::from(path),
            size_bytes,
            used: UNIX_EPOCH + Duration::from_secs(age),
        }
    }

    fn evicted(evictions: Vec<CacheEntry>) -> Vec<PathBuf> {
        evictions.into_iter().map(|entry| entry.path).collect()
    }

    /// A build directory with `bytes` of artifacts in it, named `id` in
    /// `root`.
    fn build_dir(root: &Path, id: &str, bytes: usize) -> PathBuf {
        let dir = root.join(id);
        let profile = dir.join("debug");
        fs::create_dir_all(&profile).unwrap();
        fs::write(profile.join("artifact"), vec![0_u8; bytes]).unwrap();
        dir
    }

    fn occupied(root: &Path) -> u64 {
        total_bytes(&candidate_entries(root).unwrap().entries)
    }

    /// A runner always has some job, so a root held whole was 1.4 TB the
    /// ceiling could never reclaim, and every pass answered the shortfall by
    /// evicting every warm lane directory instead. The scan has to see the
    /// lease where the lane takes it, and charge that one directory only. The
    /// alias beside it is a link to it, and is neither followed nor counted.
    #[cfg(unix)]
    #[test]
    fn a_lease_held_in_an_entry_keeps_that_entry_and_not_its_siblings() {
        let root = tempfile::tempdir().unwrap();
        let build = root.path().join("lint");
        let idle = root.path().join("usdt");
        fs::create_dir_all(&idle).unwrap();
        let lease = lease::hold(&build).expect("claim the directory the lane builds into");
        std::os::unix::fs::symlink("lint", root.path().join(consts::BUILD_ALIAS)).unwrap();

        let contents = candidate_entries(root.path()).unwrap();

        let paths = |entries: &[CacheEntry]| -> Vec<PathBuf> {
            entries.iter().map(|entry| entry.path.clone()).collect()
        };
        assert_eq!(paths(&contents.held), [build]);
        assert_eq!(paths(&contents.entries), [idle]);
        drop(lease);
    }

    /// The same tree without a holder: the guard above must not answer "held"
    /// about a directory that was merely left behind, or nothing is ever
    /// reclaimed.
    #[test]
    fn an_unheld_lease_leaves_its_entry_evictable() {
        let root = tempfile::tempdir().unwrap();
        let build = root.path().join("lint");
        drop(lease::hold(&build).expect("claim and release"));

        let contents = candidate_entries(root.path()).unwrap();

        assert!(
            contents.held.is_empty(),
            "an abandoned lease kept its entry"
        );
        assert!(contents.entries.iter().any(|entry| entry.path == build));
    }

    #[cfg(unix)]
    #[test]
    fn using_an_old_cache_refreshes_its_eviction_order() {
        let root = tempfile::tempdir().unwrap();
        let used = root.path().join("old-but-used");
        let idle = root.path().join("newer-but-idle");
        fs::create_dir_all(&idle).unwrap();
        drop(lease::hold(&idle).unwrap());
        drop(lease::hold(&used).unwrap());
        for (dir, age) in [(&used, 10), (&idle, 20)] {
            File::options()
                .write(true)
                .open(dir.join(lease::FILE))
                .unwrap()
                .set_modified(UNIX_EPOCH + Duration::from_secs(age))
                .unwrap();
        }

        drop(lease::hold(&used).unwrap());

        let contents = candidate_entries(root.path()).unwrap();
        let date = |path: &Path| {
            contents
                .entries
                .iter()
                .find(|entry| entry.path == path)
                .map(|entry| entry.used)
                .unwrap()
        };
        assert!(date(&used) > date(&idle));
    }

    /// Only a job's lease dates an entry. A directory no job ever leased is
    /// left from a layout no lane builds in any more, so it goes before any
    /// directory a lane has used, however recently something wrote into it.
    #[test]
    fn an_entry_no_job_ever_leased_goes_before_any_leased_one() {
        let root = tempfile::tempdir().unwrap();
        let leased = root.path().join("lint");
        let legacy = root.path().join("debug");
        drop(lease::hold(&leased).unwrap());
        File::options()
            .write(true)
            .open(leased.join(lease::FILE))
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(10))
            .unwrap();
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("artifact"), b"built just now").unwrap();

        let contents = candidate_entries(root.path()).unwrap();

        let date = |path: &Path| {
            contents
                .entries
                .iter()
                .find(|entry| entry.path == path)
                .map(|entry| entry.used)
                .unwrap()
        };
        assert!(date(&legacy) < date(&leased));
        assert!(
            !legacy.join(lease::FILE).exists(),
            "the scan leased a directory it only looked at"
        );
    }

    /// A job inside a virtual machine leases its build directory where the
    /// host cannot see the lock, so the heartbeat it beats into the directory
    /// is what tells the host evictor the job lives.
    #[test]
    fn a_fresh_heartbeat_inside_an_entry_keeps_it() {
        let root = tempfile::tempdir().unwrap();
        let lane = root.path().join("lint");
        fs::create_dir_all(&lane).unwrap();
        fs::write(lane.join(lease::HEARTBEAT), b"").unwrap();

        let contents = candidate_entries(root.path()).unwrap();

        assert!(
            contents.held.iter().any(|entry| entry.path == lane),
            "the directory a VM job beats into was not charged as live"
        );
        assert!(
            contents.entries.is_empty(),
            "the directory a VM job beats into was offered for eviction"
        );
    }

    /// A job that died in its virtual machine stops beating.
    #[test]
    fn a_stale_heartbeat_leaves_its_entry_evictable() {
        let root = tempfile::tempdir().unwrap();
        let lane = root.path().join("lint");
        fs::create_dir_all(&lane).unwrap();
        File::create(lane.join(lease::HEARTBEAT))
            .unwrap()
            .set_times(FileTimes::new().set_modified(
                SystemTime::now() - consts::HEARTBEAT_MAX_AGE - Duration::from_secs(1),
            ))
            .unwrap();

        let contents = candidate_entries(root.path()).unwrap();

        assert!(contents.entries.iter().any(|entry| entry.path == lane));
    }

    #[test]
    fn under_budget_deletes_nothing() {
        let entries = vec![entry("lint", 10, 1), entry("usdt", 20, 2)];

        assert!(select_evictions(entries, 30).is_empty());
    }

    #[test]
    fn over_budget_deletes_oldest_first() {
        let entries = vec![
            entry("newest", 10, 3),
            entry("oldest", 10, 1),
            entry("middle", 10, 2),
        ];
        let paths = evicted(select_evictions(entries, 0));

        assert_eq!(paths, ["oldest", "middle", "newest"].map(PathBuf::from));
    }

    #[test]
    fn deletion_stops_as_soon_as_the_budget_is_met() {
        let entries = vec![entry("oldest", 5, 1), entry("newest", 7, 2)];

        assert_eq!(select_evictions(entries, 7).len(), 1);
    }

    #[test]
    fn equal_timestamps_break_ties_by_path() {
        let entries = vec![entry("b", 1, 1), entry("a", 1, 1)];
        let paths = evicted(select_evictions(entries, 0));

        assert_eq!(paths, ["a", "b"].map(PathBuf::from));
    }

    #[test]
    fn a_cargo_home_inside_a_build_root_is_never_evicted() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let sources = target.join(".kithara-ci/review/linux-x86_64/cargo/registry/src");
        fs::create_dir_all(&sources).unwrap();
        fs::write(sources.join("lib.rs"), vec![0_u8; 100_000]).unwrap();
        let lane = build_dir(&target, "lint", 100_000);

        enforce_budget(std::slice::from_ref(&target), 0).unwrap();

        assert!(sources.join("lib.rs").is_file());
        assert!(!lane.exists());
    }

    /// Each checkout under the budget while the host they share is out of room
    /// is the state that produced `bytes_freed=0` on every pass for hours.
    #[test]
    fn the_budget_is_a_ceiling_over_every_root_together() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("one");
        let second = root.path().join("two");
        build_dir(&first, "lint", 100_000);
        build_dir(&second, "lint", 100_000);
        let budget = 150_000;
        assert!(occupied(&first) < budget, "each root is under the budget");

        enforce_budget(&[first.clone(), second.clone()], budget).unwrap();

        assert!(occupied(&first) + occupied(&second) <= budget);
    }

    /// The refused job's state, which the ceiling cannot see: caches sit under
    /// it, so an `enforce_budget` pass frees nothing, while the volume is short
    /// of what one job needs because the rest of it is spent on checkouts,
    /// toolchains and guests. That is how a job was refused over one missing
    /// gigabyte with 15.7 GB of evictable cache beside it.
    #[test]
    fn a_shortfall_is_reclaimed_even_though_the_caches_are_under_the_ceiling() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("one");
        let second = root.path().join("two");
        build_dir(&first, "lint", 100_000);
        build_dir(&second, "lint", 100_000);
        let roots = [first.clone(), second.clone()];
        let before = occupied(&first) + occupied(&second);
        enforce_budget(&roots, before * 2).unwrap();
        assert_eq!(
            occupied(&first) + occupied(&second),
            before,
            "under the ceiling the hourly pass has nothing to do"
        );
        let shortfall = before / 4;

        reclaim_at_least(&roots, shortfall).unwrap();

        let left = occupied(&first) + occupied(&second);
        assert!(
            before - left >= shortfall,
            "the shortfall of {shortfall} must be freed, {left} bytes left of {before}"
        );
        assert!(
            left > 0,
            "only the shortfall is owed; the rest of the cache is worth keeping"
        );
    }

    /// What the ceiling does with a live entry and an idle one: the live one
    /// survives, and its bytes are still spent, so the idle one is what pays.
    #[test]
    fn the_directory_a_lane_runs_from_survives_and_the_idle_one_pays() {
        let root = tempfile::tempdir().unwrap();
        let running = build_dir(root.path(), "lint", 400_000);
        let idle = build_dir(root.path(), "usdt", 400_000);
        let lane = lease::hold(&running).expect("the lane claims what it builds");

        let budget =
            occupied(root.path()) + total_bytes(&candidate_entries(root.path()).unwrap().held);
        enforce_budget(&[root.path().to_path_buf()], budget - 1).unwrap();

        assert!(
            running.join("debug/artifact").exists(),
            "the directory a lane is executing from must survive the ceiling"
        );
        assert!(
            !idle.exists(),
            "the live directory is charged against the ceiling, so the idle one is evicted"
        );
        drop(lane);
    }

    /// The scan only reads; a job that leases an entry the scan offered keeps
    /// it.
    #[test]
    fn an_entry_a_job_leases_after_the_scan_is_kept() {
        let root = tempfile::tempdir().unwrap();
        let lane = build_dir(root.path(), "lint", 1);
        let scanned = candidate_entries(root.path()).unwrap().entries;

        let job = lease::hold(&lane).unwrap();

        assert_eq!(evict(&scanned[0]).unwrap(), 0);
        assert!(lane.join("debug/artifact").exists());
        drop(job);
    }

    /// An evicted entry is moved aside before it is removed, and nothing of
    /// either is left.
    #[test]
    fn an_evicted_entry_leaves_nothing_behind() {
        let root = tempfile::tempdir().unwrap();
        build_dir(root.path(), "lint", 1);
        let scanned = candidate_entries(root.path()).unwrap().entries;

        assert!(evict(&scanned[0]).unwrap() > 0);

        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }

    /// An eviction moves an entry aside before it deletes it. One that died
    /// midway leaves the moved directory behind, and no job ever builds there
    /// to make it a candidate.
    #[test]
    fn what_an_interrupted_eviction_left_is_removed() {
        let root = tempfile::tempdir().unwrap();
        let left = root.path().join(".evicting-lint-1");
        fs::create_dir_all(left.join("debug")).unwrap();
        fs::write(left.join("debug/artifact"), b"half removed").unwrap();
        fs::write(left.join(lease::FILE), b"").unwrap();

        enforce_budget(&[root.path().to_path_buf()], u64::MAX).unwrap();

        assert!(!left.exists());
    }

    /// The eviction that moved this entry aside is still removing it.
    #[test]
    fn what_another_eviction_is_removing_is_left_to_it() {
        let root = tempfile::tempdir().unwrap();
        let left = root.path().join(".evicting-lint-1");
        fs::create_dir_all(left.join("debug")).unwrap();
        let removing = lease::evict(&left).unwrap().expect("nobody holds it");

        enforce_budget(&[root.path().to_path_buf()], u64::MAX).unwrap();

        assert!(left.join("debug").exists());
        drop(removing);
    }

    /// The roots are what an executor names beside each checkout; the
    /// checkouts themselves, and the trees inside them, are never read.
    #[cfg(unix)]
    #[test]
    fn the_build_roots_are_the_directories_beside_the_checkouts() {
        let root = tempfile::tempdir().unwrap();
        let mut expected = Vec::new();
        for slot in ["runner-a/0", "runner-b/1"] {
            let checkout = root.path().join(slot).join("disrupt/kithara");
            fs::create_dir_all(checkout.join("target/debug")).unwrap();
            fs::write(checkout.join("Cargo.toml"), b"").unwrap();
            let build_root = checkout.with_file_name("kithara.target");
            build_dir(&build_root, "lint", 1);
            expected.push(build_root);
        }
        std::os::unix::fs::symlink("lint", expected[0].join(consts::BUILD_ALIAS)).unwrap();
        build_dir(&expected[1], consts::XTASK_BUILD, 1);
        fs::create_dir_all(root.path().join("runner-c/0/disrupt/unrelated/debug")).unwrap();

        assert_eq!(build_roots(root.path()).unwrap(), expected);
    }

    /// A build alias is a link `ci lane` points at the lane's own directory
    /// once it knows the lane. Leased through before that, it claimed the last
    /// lane's directory for this job, or became a directory where the link
    /// belongs.
    #[test]
    fn a_build_alias_is_not_leased_through() {
        let root = tempfile::tempdir().unwrap();
        let alias = root.path().join(consts::BUILD_ALIAS);

        let lease = lease_target(Some(alias.clone())).unwrap();

        assert!(lease.is_none());
        assert!(
            fs::symlink_metadata(&alias).is_err(),
            "nothing stands where the alias goes"
        );
    }

    /// A sweep runs over a cache a job is building in, so a name the listing
    /// returned can be gone before it is read. Treating that as a failure
    /// aborted the whole sweep, and the budget it exists to enforce was never
    /// applied: the disk kept growing while the timer reported a failed unit.
    #[test]
    fn an_entry_a_live_build_removed_mid_sweep_is_skipped() {
        let gone = still_there::<()>(Err(io::Error::from(io::ErrorKind::NotFound)))
            .expect("a vanished entry is not a failure");
        assert!(gone.is_none(), "a vanished entry is counted as nothing");

        let refused = still_there::<()>(Err(io::Error::from(io::ErrorKind::PermissionDenied)));
        assert!(
            refused.is_err(),
            "a cache this sweep cannot read is a real failure, not a race"
        );

        let directory = tempfile::tempdir().expect("temporary directory");
        let present = still_there(fs::symlink_metadata(directory.path()))
            .expect("reading an entry that is there")
            .expect("an entry that is there is measured");
        assert!(present.file_type().is_dir());
    }
}
