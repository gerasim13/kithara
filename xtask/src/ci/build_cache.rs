#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result, bail};
use fs4::TryLockError;
use kithara_devtools::{lease, lock::FileLock};
use tracing::info;

use super::lane_build;
use crate::consts;

#[derive(Clone, Debug, Eq, PartialEq)]
struct CacheEntry {
    path: PathBuf,
    size_bytes: u64,
    modified: SystemTime,
    /// The build units inside, so the budget can take the ones no build used
    /// for longest rather than every build the entry holds.
    units: Vec<SizedUnit>,
    /// The lane slot the entry is or lies in. A job holds the slot's lock for
    /// as long as it builds there, so the removal takes that lock; the scan
    /// leaves it to any job that claims the slot meanwhile.
    slot: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SizedUnit {
    unit: lane_build::UnitUse,
    size_bytes: u64,
}

/// What the budget takes from one entry.
#[derive(Debug, Eq, PartialEq)]
enum Eviction {
    /// Every unit went, so the entry goes whole with what Cargo keeps beside
    /// its units.
    Whole(CacheEntry),
    /// The units of the entry no build used for longest.
    Units(CacheEntry, Vec<SizedUnit>),
}

impl Eviction {
    fn entry(&self) -> &CacheEntry {
        match self {
            Self::Whole(entry) | Self::Units(entry, _) => entry,
        }
    }

    fn size_bytes(&self) -> u64 {
        match self {
            Self::Whole(entry) => entry.size_bytes,
            Self::Units(_, units) => units
                .iter()
                .map(|unit| unit.size_bytes)
                .fold(0_u64, u64::saturating_add),
        }
    }
}

struct CacheContents {
    entries: Vec<CacheEntry>,
    /// Entries a live job builds in. They are charged against the ceiling and
    /// never evicted, but they do not make their siblings unevictable.
    held: Vec<CacheEntry>,
    active: bool,
    locks: Vec<FileLock>,
}

struct DirectoryScan {
    bytes: u64,
    last_used: Option<SystemTime>,
    active: bool,
    locks: Vec<FileLock>,
}

/// The least recently used build units across every entry, until what is left
/// fits the budget.
///
/// A unit is dated by its own last use. What an entry keeps beside its units
/// is dated by its newest one, so it goes only with the last of them, and the
/// entry goes whole with it. An entry with no units is that remainder alone.
fn select_evictions(entries: Vec<CacheEntry>, budget_bytes: u64) -> Vec<Eviction> {
    struct Item<'a> {
        used: SystemTime,
        path: &'a Path,
        /// The remainder sorts after its entry's units when they tie.
        remainder: bool,
        hash: &'a str,
        entry: usize,
        unit: Option<usize>,
        size_bytes: u64,
    }

    let mut remaining_bytes: u128 = entries
        .iter()
        .map(|entry| u128::from(entry.size_bytes))
        .sum();
    let budget_bytes = u128::from(budget_bytes);
    if remaining_bytes <= budget_bytes {
        return Vec::new();
    }

    let mut items = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let mut newest = entry.modified;
        let mut unit_bytes = 0_u64;
        for (position, sized) in entry.units.iter().enumerate() {
            newest = newest.max(sized.unit.used);
            unit_bytes = unit_bytes.saturating_add(sized.size_bytes);
            items.push(Item {
                used: sized.unit.used,
                path: &entry.path,
                remainder: false,
                hash: &sized.unit.hash,
                entry: index,
                unit: Some(position),
                size_bytes: sized.size_bytes,
            });
        }
        items.push(Item {
            used: newest,
            path: &entry.path,
            remainder: true,
            hash: "",
            entry: index,
            unit: None,
            size_bytes: entry.size_bytes.saturating_sub(unit_bytes),
        });
    }
    items.sort_by(|left, right| {
        left.used
            .cmp(&right.used)
            .then_with(|| left.path.cmp(right.path))
            .then_with(|| left.remainder.cmp(&right.remainder))
            .then_with(|| left.hash.cmp(right.hash))
    });

    let mut order = Vec::new();
    let mut seen = BTreeSet::new();
    let mut whole = BTreeSet::new();
    let mut taken: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for item in items {
        if remaining_bytes <= budget_bytes {
            break;
        }
        remaining_bytes = remaining_bytes.saturating_sub(u128::from(item.size_bytes));
        if seen.insert(item.entry) {
            order.push(item.entry);
        }
        match item.unit {
            Some(position) => taken.entry(item.entry).or_default().push(position),
            None => {
                whole.insert(item.entry);
            }
        }
    }

    let mut entries: Vec<Option<CacheEntry>> = entries.into_iter().map(Some).collect();
    order
        .into_iter()
        .filter_map(|index| {
            let entry = entries.get_mut(index)?.take()?;
            let positions = taken.remove(&index).unwrap_or_default();
            // An entry left with no unit holds nothing a build could reuse.
            if whole.contains(&index) || positions.len() == entry.units.len() {
                return Some(Eviction::Whole(entry));
            }
            let units = positions
                .into_iter()
                .filter_map(|position| entry.units.get(position).cloned())
                .collect();
            Some(Eviction::Units(entry, units))
        })
        .collect()
}

/// The budget is what the host can afford in total, not what one checkout may
/// keep.
///
/// Applied per directory it never fires on a machine that is running out:
/// three checkouts holding 14, 7 and 22 GB were each under a 25 GB budget, so
/// every hourly pass reported `bytes_freed=0` while the volume they share sat
/// at `Aggressive` and jobs were already being refused. A checkout an active
/// job holds cannot be evicted, but the room it occupies is still spent, so it
/// is charged against the ceiling rather than excused from it.
pub(crate) fn enforce_budget(target_dirs: &[PathBuf], budget_bytes: u64) -> Result<()> {
    let (candidates, held_bytes, _locks) = collect(target_dirs, budget_bytes)?;
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
    let (candidates, held_bytes, _locks) = collect(target_dirs, bytes_needed)?;
    let keep = total_bytes(&candidates).saturating_sub(bytes_needed);
    evict_to_budget(candidates, held_bytes, keep.saturating_add(held_bytes))
}

fn collect(
    target_dirs: &[PathBuf],
    budget_bytes: u64,
) -> Result<(Vec<CacheEntry>, u64, Vec<FileLock>)> {
    let mut target_dirs = target_dirs.to_vec();
    target_dirs.sort();
    let mut candidates = Vec::new();
    let mut held_bytes = 0_u64;
    let mut locks = Vec::new();
    for target_dir in &target_dirs {
        let contents = candidate_entries(target_dir)?;
        locks.extend(contents.locks);
        let bytes = total_bytes(&contents.entries);
        let held = total_bytes(&contents.held);
        if contents.active {
            held_bytes = held_bytes.saturating_add(bytes).saturating_add(held);
            info!(
                path = %target_dir.display(),
                bytes_before = bytes.saturating_add(held),
                bytes_freed = 0,
                budget_bytes,
                "keeping active build cache"
            );
            continue;
        }
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
    Ok((candidates, held_bytes, locks))
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
    for eviction in select_evictions(candidates, budget_bytes.saturating_sub(held_bytes)) {
        bytes_freed = bytes_freed.saturating_add(evict(&eviction)?);
    }
    info!(
        bytes_before,
        bytes_freed, held_bytes, budget_bytes, "build cache budget enforced"
    );
    Ok(())
}

/// Removes what one eviction names and returns the bytes it held.
///
/// A unit goes with every file Cargo names after it, the way pruning at a
/// claim removes one: Cargo rebuilds a unit it finds missing, and the units
/// that depend on it after it. Units a build still uses are touched by it, so
/// they are newer than any unit taken before them.
fn evict(eviction: &Eviction) -> Result<u64> {
    let entry = eviction.entry();
    let _slot = match &entry.slot {
        Some(slot) => match slot_lock(slot)? {
            (true, _) => {
                info!(
                    path = %entry.path.display(),
                    "keeping the build cache of a lane slot a job claimed after the scan"
                );
                return Ok(0);
            }
            (false, lock) => lock,
        },
        None => None,
    };
    let bytes = eviction.size_bytes();
    match eviction {
        Eviction::Whole(entry) => {
            info!(path = %entry.path.display(), bytes, "evicting build cache");
            fs::remove_dir_all(&entry.path)
                .with_context(|| format!("removing build cache entry {}", entry.path.display()))?;
        }
        Eviction::Units(entry, units) => {
            let mut by_profile: BTreeMap<&Path, BTreeSet<&str>> = BTreeMap::new();
            for sized in units {
                by_profile
                    .entry(sized.unit.profile.as_path())
                    .or_default()
                    .insert(sized.unit.hash.as_str());
            }
            info!(
                path = %entry.path.display(),
                units = units.len(),
                bytes,
                "evicting the build units no build used for longest"
            );
            for (profile, hashes) in &by_profile {
                lane_build::remove_units(profile, hashes)?;
            }
        }
    }
    Ok(bytes)
}

/// What a listed entry still is, or nothing when it is already gone.
///
/// The cache being measured is one a job may be building in, and a compiler
/// writes a temporary file and removes it again. A name the listing returned
/// and the build has since deleted is that race, not a broken cache: there is
/// nothing left to count or to reclaim. Any other failure is a real one, and
/// stopping the sweep on it is why the budget went unenforced.
fn still_there(metadata: io::Result<fs::Metadata>) -> io::Result<Option<fs::Metadata>> {
    match metadata {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn candidate_entries(target_dir: &Path) -> Result<CacheContents> {
    let Some(parent) = target_dir.parent().filter(|_| target_dir.is_absolute()) else {
        bail!(
            "refusing to inspect unsafe build cache path {}",
            target_dir.display()
        );
    };
    let metadata = fs::symlink_metadata(target_dir)
        .with_context(|| format!("reading build cache metadata for {}", target_dir.display()))?;
    if !metadata.file_type().is_dir() {
        bail!(
            "build cache path is not a directory: {}",
            target_dir.display()
        );
    }
    let entries = fs::read_dir(target_dir)
        .with_context(|| format!("reading build cache {}", target_dir.display()))?;
    let mut contents = CacheContents {
        entries: Vec::new(),
        held: Vec::new(),
        // A target a live job leased is active even between compilations,
        // when no `.cargo-lock` is held.
        active: lease_is_held(target_dir, FileLock::try_exclusive),
        locks: Vec::new(),
    };
    // On GitLab the target is a slot's `cargo` directory, with the slot's lock
    // beside the slot. The scan only asks whether a job holds it.
    let (held, lock) = slot_lock(parent)?;
    contents.active |= held;
    let target_slot = lock.map(|_| parent.to_path_buf());
    for entry in entries {
        let entry = entry
            .with_context(|| format!("reading an entry in build cache {}", target_dir.display()))?;
        let path = entry.path();
        let Some(metadata) = still_there(fs::symlink_metadata(&path))
            .with_context(|| format!("reading build cache metadata for {}", path.display()))?
        else {
            continue;
        };
        if metadata.file_type().is_dir() && is_hidden(&path) {
            continue;
        }
        if !metadata.file_type().is_dir() {
            if metadata.file_type().is_file()
                && path.file_name() == Some(OsStr::new(consts::TARGET_HEARTBEAT_FILE))
            {
                contents.active |= heartbeat_is_fresh(&path, &metadata);
            }
            continue;
        }
        let modified = metadata
            .modified()
            .with_context(|| format!("reading modification time for {}", path.display()))?;
        // A free slot is only read here, so a job that claims it during the
        // scan builds in it rather than in a new, cold one; the removal takes
        // the lock. Outside a slot the scan keeps what it locked, so no build
        // starts in what the pass may remove.
        let (slot_held, free) = slot_lock(&path)?;
        let slot = free.map(|_| path.clone()).or_else(|| target_slot.clone());
        let scan = scan_directory(&path)?;
        if slot.is_none() {
            contents.locks.extend(scan.locks);
        }
        let live = scan.active || slot_held;
        let entry = CacheEntry {
            units: if live {
                Vec::new()
            } else {
                entry_units(&path)?
            },
            size_bytes: scan.bytes,
            modified: scan.last_used.map_or(modified, |used| used.max(modified)),
            slot,
            path,
        };
        if live {
            contents.held.push(entry);
        } else {
            contents.entries.push(entry);
        }
    }
    Ok(contents)
}

/// The build units in `entry`, each with the room its files take.
fn entry_units(entry: &Path) -> Result<Vec<SizedUnit>> {
    let units = lane_build::unit_uses(entry)?;
    let profiles: BTreeSet<&Path> = units.iter().map(|unit| unit.profile.as_path()).collect();
    let mut sizes: BTreeMap<&Path, BTreeMap<String, u64>> = BTreeMap::new();
    for profile in profiles {
        let by_hash = sizes.entry(profile).or_default();
        for (hash, path) in lane_build::unit_paths(profile)? {
            let bytes = scan_directory(&path)?.bytes;
            let size = by_hash.entry(hash).or_default();
            *size = size.saturating_add(bytes);
        }
    }
    let sized = units
        .iter()
        .map(|unit| SizedUnit {
            size_bytes: sizes
                .get(unit.profile.as_path())
                .and_then(|by_hash| by_hash.get(unit.hash.as_str()))
                .copied()
                .unwrap_or_default(),
            unit: unit.clone(),
        })
        .collect();
    Ok(sized)
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

fn scan_directory(path: &Path) -> Result<DirectoryScan> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("reading build cache metadata for {}", path.display()))?;
    if !metadata.file_type().is_dir() {
        if metadata.file_type().is_file() && path.file_name() == Some(OsStr::new(".cargo-lock")) {
            let (active, lock) = cargo_lock(path)?;
            return Ok(DirectoryScan {
                bytes: allocated_bytes(&metadata),
                last_used: None,
                active,
                locks: lock.into_iter().collect(),
            });
        }
        // A lane names its own build directory below the one this scan starts
        // from — on Linux the volume root is `/cache/target` and the lane
        // builds into `/cache/target/flash-off` — so its claim is a lease file
        // in a child. Asking only the root left the claim unseen and the live
        // directory evictable, which is how an hourly pass removed the test
        // binaries of a running job and 1642 of its tests failed to exec.
        if metadata.file_type().is_file() && path.file_name() == Some(OsStr::new(lease::FILE)) {
            return Ok(DirectoryScan {
                bytes: allocated_bytes(&metadata),
                last_used: Some(metadata.modified()?),
                active: lease_file_is_held(path, FileLock::try_exclusive),
                locks: Vec::new(),
            });
        }
        return Ok(DirectoryScan {
            bytes: allocated_bytes(&metadata),
            last_used: None,
            active: false,
            locks: Vec::new(),
        });
    }

    let entries = fs::read_dir(path)
        .with_context(|| format!("reading build cache directory {}", path.display()))?;
    let mut bytes = allocated_bytes(&metadata);
    let mut last_used = None;
    let mut active = false;
    let mut locks = Vec::new();
    for entry in entries {
        let entry =
            entry.with_context(|| format!("reading an entry in build cache {}", path.display()))?;
        let scan = scan_directory(&entry.path())?;
        bytes = bytes.saturating_add(scan.bytes);
        last_used = last_used.max(scan.last_used);
        active |= scan.active;
        locks.extend(scan.locks);
    }
    Ok(DirectoryScan {
        bytes,
        last_used,
        active,
        locks,
    })
}

fn cargo_lock(path: &Path) -> Result<(bool, Option<FileLock>)> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("opening Cargo build lock {}", path.display()))?;
    held_lock(file, path, "Cargo build lock")
}

/// The lock beside a lane slot. Held, a job builds in the slot; free, the pass
/// takes it so no job starts in a slot the pass may remove. A directory without
/// one is no lane slot.
fn slot_lock(slot: &Path) -> Result<(bool, Option<FileLock>)> {
    let path = lane_build::lock_of(slot);
    let file = match OpenOptions::new().read(true).write(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((false, None)),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("opening lane slot lock {}", path.display()));
        }
    };
    held_lock(file, &path, "lane slot lock")
}

/// Whether another holder has the lock, or the lock itself when nobody does.
fn held_lock(file: File, path: &Path, what: &str) -> Result<(bool, Option<FileLock>)> {
    match FileLock::try_exclusive(file) {
        Ok(lock) => Ok((false, Some(lock))),
        Err(TryLockError::WouldBlock) => Ok((true, None)),
        Err(TryLockError::Error(error)) => {
            Err(error).with_context(|| format!("checking {what} {}", path.display()))
        }
    }
}

#[cfg(unix)]
fn allocated_bytes(metadata: &fs::Metadata) -> u64 {
    metadata.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn allocated_bytes(metadata: &fs::Metadata) -> u64 {
    metadata.len()
}

/// Whether a live job holds `directory`, asked with the request that sees the
/// kind of holder in question: `try_lock_shared` for a checkout, `try_lock`
/// for a Cargo target.
///
/// `.cargo-lock` cannot answer that: Cargo holds it only while it compiles, so
/// a checkout or shared target whose tests are already running looks
/// abandoned. A reclaim that believed it deleted a live `target` out from
/// under a sibling job, and the 1869 tests that then failed to exec their own
/// binaries looked like the product breaking rather than the CI eating itself.
///
/// A checkout owner takes the lease exclusively, so a shared request already
/// cannot be granted while it holds, and it leaves concurrent observers alone.
/// Asking exclusively made the question itself exclusive, so two observers
/// answered "held" about each other while no job held anything — measured on
/// Linux with four observers and no owner, 37694 of 80000 answers were that
/// invention. Target holders take the lease shared so several coexist, so only
/// an exclusive request sees them at all.
fn lease_is_held(directory: &Path, ask: fn(File) -> Result<FileLock, TryLockError>) -> bool {
    lease_file_is_held(&directory.join(lease::FILE), ask)
}

/// [`lease_is_held`] for a lease file the scan already found, rather than one
/// named from the directory expected to hold it.
fn lease_file_is_held(path: &Path, ask: fn(File) -> Result<FileLock, TryLockError>) -> bool {
    let Ok(file) = OpenOptions::new().read(true).write(true).open(path) else {
        return false;
    };
    // Held by a live job, or unreadable — either way, not ours to remove.
    ask(file).is_err()
}

fn heartbeat_is_fresh(path: &Path, metadata: &fs::Metadata) -> bool {
    let Ok(modified) = metadata.modified() else {
        return true;
    };
    let Ok(age) = SystemTime::now().duration_since(modified) else {
        return true;
    };
    if age <= consts::HEARTBEAT_MAX_AGE {
        return true;
    }
    let _ = fs::remove_file(path);
    false
}

/// Claims `CARGO_TARGET_DIR` for the life of this process, if one is named.
///
/// The checkout lease cannot protect it: on Linux runners the target is a
/// per-runner Docker volume the host budgets directly, and a lease on the
/// checkout says nothing about a directory outside it. A lane that builds into
/// a directory of its own claims that one where it names it, so both claims are
/// the one protocol [`lease`] owns and [`lease_is_held`] asks about.
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

/// Every `target` directory under `root`, so a caller can hand them to
/// [`enforce_budget`]. Shared with the environment gate: refusing a job is only
/// honest once these have been reclaimed.
///
/// A checkout a job is working in is listed like any other. Protection belongs
/// one level down, on the build directory the lane itself claims: skipping the
/// whole checkout took its caches out of the budget's sight before it could
/// weigh them, so on a host whose only checkout is the live one the ceiling had
/// nothing at all to act on and never reclaimed the space it exists to hold.
/// Listed here, the cache a lane is running from is kept by
/// [`candidate_entries`] and its bytes are charged against the ceiling, which is
/// what leaves the idle caches beside it payable.
pub(crate) fn persistent_target_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }

    let mut pending = vec![root.to_path_buf()];
    let mut targets = Vec::new();
    while let Some(directory) = pending.pop() {
        if directory.join("Cargo.toml").is_file() {
            // `target-stress` belongs here too: it is target-dir sized, the
            // repo's own tooling creates it, and no other pass owns it. It is
            // only safe to reclaim because the lane that builds into it holds
            // the lease `candidate_entries` asks about.
            for name in ["target", "target-flash-off", "target-stress"] {
                let path = directory.join(name);
                let metadata = match fs::symlink_metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(error).with_context(|| format!("reading {}", path.display()));
                    }
                };
                if metadata.file_type().is_dir() {
                    targets.push(path);
                }
            }
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
            if entry.file_name().to_string_lossy().starts_with('.') {
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
    targets.sort();
    Ok(targets)
}

/// Build directories kept in the executor cache rather than in a checkout.
///
/// GitLab checkouts are cleaned between jobs. Their targets live below the
/// mounted cache root, one per runner slot, so the host budget must discover
/// them without walking Cargo homes and compiler caches beside them.
pub(crate) fn cached_target_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let slots = root.join(consts::TARGET_SLOT_CACHE_NAMESPACE);
    if !slots.is_dir() {
        return Ok(Vec::new());
    }

    let mut targets = Vec::new();
    for entry in
        fs::read_dir(&slots).with_context(|| format!("reading build cache {}", slots.display()))?
    {
        let entry = entry.with_context(|| format!("reading build cache {}", slots.display()))?;
        if entry.file_type()?.is_dir() {
            // Target snapshots give each job a private parent. Keep accepting
            // the pre-snapshot flat slot layout until its old directories age
            // out, but charge and reclaim the writable Cargo directory.
            let path = entry.path();
            let cargo = path.join("cargo");
            targets.push(if cargo.is_dir() { cargo } else { path });
        }
    }
    targets.sort();
    Ok(targets)
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{FileTimes, OpenOptions},
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        thread,
        time::{Duration, UNIX_EPOCH},
    };

    use super::*;

    fn entry(path: &str, size_bytes: u64, age: u64) -> CacheEntry {
        CacheEntry {
            path: PathBuf::from(path),
            size_bytes,
            modified: UNIX_EPOCH + Duration::from_secs(age),
            units: Vec::new(),
            slot: None,
        }
    }

    fn evicted(evictions: Vec<Eviction>) -> Vec<PathBuf> {
        evictions
            .into_iter()
            .map(|eviction| eviction.entry().path.clone())
            .collect()
    }

    /// The Linux volume root is scanned, and the lane builds one level down
    /// and claims that level. The scan has to see the claim where the lane
    /// makes it, and it has to charge that one directory rather than the root:
    /// a runner always has some job, so a root held whole was 1.4 TB the
    /// ceiling could never reclaim, and every hourly pass answered the
    /// shortfall by evicting every warm lane directory instead.
    #[test]
    fn a_lease_held_in_a_child_keeps_that_child_and_not_its_siblings() {
        let root = tempfile::tempdir().unwrap();
        let build = root.path().join("flash-off");
        let idle = root.path().join("flash-on");
        fs::create_dir_all(&idle).unwrap();
        let lease = lease::hold(&build).expect("claim the directory the lane builds into");

        let contents = candidate_entries(root.path()).unwrap();

        assert!(
            !contents.active,
            "one live child made the whole root unevictable"
        );
        assert!(
            contents.held.iter().any(|entry| entry.path == build),
            "the scan did not see the lease the lane holds in {}",
            build.display()
        );
        assert!(
            contents.entries.iter().any(|entry| entry.path == idle),
            "a directory nobody holds stayed out of the candidates"
        );
        assert!(
            contents.entries.iter().all(|entry| entry.path != build),
            "the directory a lane is building in was offered for eviction"
        );
        drop(lease);
    }

    #[cfg(unix)]
    #[test]
    fn using_an_old_cache_refreshes_its_eviction_order() {
        let root = tempfile::tempdir().unwrap();
        let used = root.path().join("old-but-used");
        let idle = root.path().join("newer-but-idle");
        fs::create_dir_all(&used).unwrap();
        fs::create_dir_all(&idle).unwrap();
        let old = UNIX_EPOCH + Duration::from_secs(10);
        let newer = UNIX_EPOCH + Duration::from_secs(20);
        drop(lease::hold(&used).unwrap());
        File::options()
            .write(true)
            .open(used.join(lease::FILE))
            .unwrap()
            .set_modified(old)
            .unwrap();
        File::open(&idle).unwrap().set_modified(newer).unwrap();
        drop(lease::hold(&used).unwrap());
        // A lease beats into its directory, which moves the directory's own
        // date; set it back so only the lease can tell the two apart.
        File::open(&used).unwrap().set_modified(old).unwrap();

        let contents = candidate_entries(root.path()).unwrap();
        assert!(!contents.active);
        let used_entry = contents
            .entries
            .iter()
            .find(|entry| entry.path == used)
            .unwrap();
        let idle_entry = contents
            .entries
            .iter()
            .find(|entry| entry.path == idle)
            .unwrap();
        assert!(used_entry.modified > idle_entry.modified);
    }

    /// The same tree without a holder: the guard above must not answer "held"
    /// about a directory that was merely left behind, or nothing is ever
    /// reclaimed.
    #[test]
    fn an_unheld_lease_in_a_child_leaves_the_root_evictable() {
        let root = tempfile::tempdir().unwrap();
        let build = root.path().join("flash-off");
        drop(lease::hold(&build).expect("claim and release"));

        let contents = candidate_entries(root.path()).unwrap();

        assert!(!contents.active, "an abandoned lease kept the root");
    }

    /// Nobody holds this lease, so every "held" answer is invented. Observers
    /// must not invent one about each other, which is what asking for the
    /// owner's own exclusive lock did.
    ///
    /// The invention needs `flock` to conflict between two descriptors of one
    /// process. That is Linux behaviour and what CI runs on; on macOS the same
    /// pair never conflicts, so this test cannot fail there.
    #[test]
    fn concurrent_observers_do_not_invent_a_checkout_holder() {
        let directory = tempfile::tempdir().unwrap();
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(directory.path().join(lease::FILE))
            .unwrap();
        let checkout = Arc::new(directory.path().to_path_buf());
        let invented = Arc::new(AtomicU64::new(0));

        let observers: Vec<_> = (0..4)
            .map(|_| {
                let checkout = Arc::clone(&checkout);
                let invented = Arc::clone(&invented);
                thread::spawn(move || {
                    for _ in 0..2_000 {
                        if lease_is_held(&checkout, FileLock::try_shared) {
                            invented.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                })
            })
            .collect();
        for observer in observers {
            observer.join().unwrap();
        }

        assert_eq!(
            invented.load(Ordering::Relaxed),
            0,
            "an observer answered for a job that never took the lease"
        );
    }

    #[test]
    fn under_budget_deletes_nothing() {
        let entries = vec![entry("debug", 10, 1), entry("release", 20, 2)];

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
    fn a_cargo_home_inside_a_build_directory_is_never_evicted() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let sources = target.join(".kithara-ci/review/linux-x86_64/cargo/registry/src");
        fs::create_dir_all(&sources).unwrap();
        fs::write(sources.join("lib.rs"), vec![0_u8; 100_000]).unwrap();
        fs::create_dir_all(target.join("debug")).unwrap();
        fs::write(target.join("debug/artifact"), vec![0_u8; 100_000]).unwrap();

        enforce_budget(std::slice::from_ref(&target), 0).unwrap();

        assert!(sources.join("lib.rs").is_file());
        assert!(!target.join("debug").exists());
    }

    /// Each checkout under the budget while the host they share is out of room
    /// is the state that produced `bytes_freed=0` on every pass for hours.
    #[test]
    fn the_budget_is_a_ceiling_over_every_checkout_together() {
        let root = tempfile::tempdir().unwrap();
        let first = checkout_target(root.path(), "one", 100_000);
        let second = checkout_target(root.path(), "two", 100_000);
        let budget = 150_000;
        assert!(
            occupied(&first) < budget,
            "each checkout is under the budget"
        );

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
        let first = checkout_target(root.path(), "one", 100_000);
        let second = checkout_target(root.path(), "two", 100_000);
        let targets = [first.clone(), second.clone()];
        let before = occupied(&first) + occupied(&second);
        enforce_budget(&targets, before * 2).unwrap();
        assert_eq!(
            occupied(&first) + occupied(&second),
            before,
            "under the ceiling the hourly pass has nothing to do"
        );
        let shortfall = before / 4;

        reclaim_at_least(&targets, shortfall).unwrap();

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

    fn checkout_target(root: &Path, name: &str, bytes: usize) -> PathBuf {
        let target = root.join(name);
        let profile = target.join("debug");
        fs::create_dir_all(&profile).unwrap();
        fs::write(profile.join("artifact"), vec![0_u8; bytes]).unwrap();
        target
    }

    fn occupied(target: &Path) -> u64 {
        total_bytes(&candidate_entries(target).unwrap().entries)
    }

    #[test]
    fn an_active_cargo_profile_defers_eviction() {
        let directory = tempfile::tempdir().unwrap();
        let profile = directory.path().join("debug");
        fs::create_dir(&profile).unwrap();
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(profile.join(".cargo-lock"))
            .unwrap();
        let lock = FileLock::try_exclusive(file).unwrap();

        let contents = candidate_entries(directory.path()).unwrap();
        assert!(
            contents.held.iter().any(|entry| entry.path == profile),
            "the profile a compilation holds was not charged as live"
        );
        assert!(
            contents.entries.is_empty(),
            "the profile a compilation holds was offered for eviction"
        );
        drop(lock);
    }

    /// A run that is only executing its built tests holds no
    /// `.cargo-lock`; the root lease is what says a job still lives in this
    /// target between compilations.
    #[test]
    fn a_leased_target_defers_eviction() {
        let directory = tempfile::tempdir().unwrap();
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.path().join(lease::FILE))
            .unwrap();
        let held = FileLock::try_shared(file).unwrap();

        assert!(candidate_entries(directory.path()).unwrap().active);
        drop(held);
    }

    /// A lease file nobody holds is a leftover, not a claim.
    #[test]
    fn a_released_lease_leaves_the_target_evictable() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(lease::FILE), b"").unwrap();

        assert!(!candidate_entries(directory.path()).unwrap().active);
    }

    #[test]
    fn a_fresh_cross_vm_heartbeat_defers_eviction() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(consts::TARGET_HEARTBEAT_FILE), b"").unwrap();

        assert!(candidate_entries(directory.path()).unwrap().active);
    }

    #[test]
    fn a_stale_cross_vm_heartbeat_leaves_the_target_evictable() {
        let directory = tempfile::tempdir().unwrap();
        let heartbeat = directory.path().join(consts::TARGET_HEARTBEAT_FILE);
        let file = File::create(&heartbeat).unwrap();
        file.set_times(
            FileTimes::new().set_modified(
                SystemTime::now() - consts::HEARTBEAT_MAX_AGE - Duration::from_secs(1),
            ),
        )
        .unwrap();

        assert!(!candidate_entries(directory.path()).unwrap().active);
        assert!(!heartbeat.exists());
    }

    /// The claim a lane takes and the question a reclaim asks are one protocol,
    /// so the holder has to be the real one, not a lock this test rolled itself.
    #[test]
    fn a_lane_holding_its_build_directory_keeps_it() {
        let directory = tempfile::tempdir().unwrap();
        let held = lease::hold(directory.path()).expect("hold the build directory");

        assert!(candidate_entries(directory.path()).unwrap().active);

        drop(held);
        assert!(!candidate_entries(directory.path()).unwrap().active);
    }

    /// One build unit the way Cargo lays it out in `profile`, last used at
    /// `used` and holding `bytes` of artifacts. Returns every file it wrote.
    fn unit(
        profile: &Path,
        package: &str,
        hash: &str,
        used: SystemTime,
        bytes: usize,
    ) -> Vec<PathBuf> {
        let fingerprint = profile
            .join(".fingerprint")
            .join(format!("{package}-{hash}"));
        let stamp = fingerprint.join(format!("lib-{package}"));
        let artifact = profile
            .join("deps")
            .join(format!("lib{package}-{hash}.rlib"));
        fs::create_dir_all(&fingerprint).unwrap();
        fs::create_dir_all(artifact.parent().unwrap()).unwrap();
        fs::write(&stamp, package).unwrap();
        fs::write(&artifact, vec![0_u8; bytes]).unwrap();
        File::options()
            .write(true)
            .open(&stamp)
            .unwrap()
            .set_modified(used)
            .unwrap();
        vec![stamp, artifact]
    }

    /// A lane slot under `lanes`, with the lock beside it a job would take.
    fn slot(lanes: &Path, name: &str) -> PathBuf {
        let slot = lanes.join(name);
        fs::create_dir_all(&slot).unwrap();
        fs::write(lane_build::lock_of(&slot), b"").unwrap();
        slot
    }

    fn slot_lock_file(slot: &Path) -> File {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(lane_build::lock_of(slot))
            .unwrap()
    }

    /// A slot holds many builds' worth of units, and only some of them are
    /// stale. Evicting the oldest slot whole took the latest build of a lane
    /// with it — the trusted slot `main` builds in is the one used least
    /// often, so every pass left `main` to rebuild from nothing — while the
    /// units a busier slot stopped using stayed. The budget weighs units,
    /// least recently used first, across every slot.
    #[test]
    fn the_budget_evicts_the_least_recently_used_units_across_slots() {
        let lanes = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let rarely = slot(lanes.path(), "trusted-lane-test-0");
        let often = slot(lanes.path(), "review-lane-test-0");
        let main = unit(
            &rarely.join("debug"),
            "main",
            "0000000000000001",
            now - 2 * consts::DAY,
            100_000,
        );
        let stale = unit(
            &often.join("debug"),
            "stale",
            "0000000000000002",
            now - 3 * consts::DAY,
            100_000,
        );
        let branch = unit(
            &often.join("debug"),
            "branch",
            "0000000000000003",
            now,
            100_000,
        );
        let total = occupied(lanes.path());

        enforce_budget(&[lanes.path().to_path_buf()], total - 1).unwrap();

        assert!(
            stale.iter().all(|path| !path.exists()),
            "the unit no build used for longest outlived the budget"
        );
        assert!(
            main.iter().all(|path| path.exists()),
            "the latest build of the slot used least often went with the stale units"
        );
        assert!(
            branch.iter().all(|path| path.exists()),
            "a unit the latest build used went while an older one was there to take"
        );
    }

    /// Every unit gone leaves nothing a build could reuse, so the slot goes
    /// whole; its lock stays, so no job locks a file the budget deleted.
    #[test]
    fn a_slot_whose_units_all_go_is_removed_and_its_lock_stays() {
        let lanes = tempfile::tempdir().unwrap();
        let idle = slot(lanes.path(), "review-lane-test-0");
        unit(
            &idle.join("debug"),
            "idle",
            "0000000000000001",
            SystemTime::now() - consts::DAY,
            100_000,
        );

        enforce_budget(&[lanes.path().to_path_buf()], 0).unwrap();

        assert!(!idle.exists(), "a slot with no unit left stayed");
        assert!(
            lane_build::lock_of(&idle).exists(),
            "the slot's lock went with it"
        );
    }

    /// A job that finds every slot locked builds in a new one, from nothing.
    /// The pass held the lock of every free slot for as long as it scanned —
    /// minutes over a fleet's slots — so each job that started meanwhile left
    /// another cold slot behind. The scan only reads; the removal locks.
    #[test]
    fn a_slot_a_job_holds_is_kept_and_a_free_one_stays_claimable_while_scanned() {
        let lanes = tempfile::tempdir().unwrap();
        let busy = slot(lanes.path(), "review-lane-test-0");
        let free = slot(lanes.path(), "review-lane-test-1");
        fs::create_dir_all(busy.join("debug")).unwrap();
        fs::create_dir_all(free.join("debug")).unwrap();
        let job = FileLock::try_exclusive(slot_lock_file(&busy)).unwrap();

        let contents = candidate_entries(lanes.path()).unwrap();

        assert!(
            contents.held.iter().any(|entry| entry.path == busy),
            "the slot a job holds was not charged as live"
        );
        assert!(
            contents.entries.iter().all(|entry| entry.path != busy),
            "the slot a job holds was offered for eviction"
        );
        assert!(
            contents.entries.iter().any(|entry| entry.path == free),
            "a slot nobody holds stayed out of the candidates"
        );
        assert!(
            FileLock::try_exclusive(slot_lock_file(&free)).is_ok(),
            "a job could not claim a free slot the pass was only reading"
        );
        drop((contents, job));
    }

    /// The scan no longer keeps a slot from a job, so the removal asks again:
    /// a slot a job claimed after the scan is the job's, whatever the scan
    /// found in it.
    #[test]
    fn a_slot_a_job_claims_after_the_scan_keeps_its_units() {
        let lanes = tempfile::tempdir().unwrap();
        let claimed = slot(lanes.path(), "review-lane-test-0");
        let build = unit(
            &claimed.join("debug"),
            "build",
            "0000000000000001",
            SystemTime::now() - consts::DAY,
            100_000,
        );
        let (candidates, held_bytes, locks) = collect(&[lanes.path().to_path_buf()], 0).unwrap();
        let job = FileLock::try_exclusive(slot_lock_file(&claimed))
            .expect("the scan left the slot claimable");

        evict_to_budget(candidates, held_bytes, 0).unwrap();

        assert!(
            build.iter().all(|path| path.exists()),
            "the pass removed units from a slot a job had claimed"
        );
        drop((locks, job));
    }

    /// On GitLab the pass starts inside the slot, at its `cargo` directory, and
    /// the slot's lock is beside the slot, one level up.
    #[test]
    fn a_gitlab_slot_is_active_while_its_job_holds_the_lock_beside_it() {
        let slots = tempfile::tempdir().unwrap();
        let slot = slot(slots.path(), "review-macos-aarch64-lane-apple-lint-0");
        let cargo = slot.join("cargo");
        fs::create_dir_all(cargo.join("debug")).unwrap();
        let job = FileLock::try_exclusive(slot_lock_file(&slot)).unwrap();

        assert!(
            candidate_entries(&cargo).unwrap().active,
            "the slot a job holds was offered for eviction"
        );

        drop(job);
        let contents = candidate_entries(&cargo).unwrap();
        assert!(!contents.active, "a slot no job holds stays reclaimable");
        assert!(
            FileLock::try_exclusive(slot_lock_file(&slot)).is_ok(),
            "a job could not claim a free slot the pass was only reading"
        );
        drop(contents);
    }

    /// The stress lane builds into a directory of its own, and a build cache no
    /// budget names is one the host can never get the space back from.
    #[test]
    fn the_stress_build_directory_is_a_cache_the_budget_owns() {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("disrupt/kithara");
        fs::create_dir_all(&checkout).unwrap();
        fs::write(checkout.join("Cargo.toml"), b"").unwrap();
        for name in ["target", "target-flash-off", "target-stress"] {
            checkout_target(&checkout, name, 1);
        }

        let targets = persistent_target_dirs(root.path()).unwrap();

        assert_eq!(
            targets,
            vec![
                checkout.join("target"),
                checkout.join("target-flash-off"),
                checkout.join("target-stress"),
            ]
        );
    }

    #[test]
    fn persistent_runner_slots_are_build_caches_the_budget_owns() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("target-slots/review-linux-aarch64-slot-0");
        let second = root.path().join("target-slots/review-linux-aarch64-slot-1");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::create_dir_all(root.path().join("review/linux-aarch64/cargo/registry")).unwrap();

        assert_eq!(cached_target_dirs(root.path()).unwrap(), [first, second]);
    }

    /// A checkout a job holds still has to answer to the ceiling.
    ///
    /// The lane's own build directory is the one it must not lose, and its own
    /// claim says so. The idle siblings beside it belong to lanes that have
    /// finished, and on a host whose only checkout is this one they are the only
    /// space the budget can ever get back — so they have to reach
    /// [`enforce_budget`], which only ever sees what this returns.
    #[test]
    fn a_leased_checkout_still_offers_the_caches_no_lane_is_using() {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("disrupt/kithara");
        fs::create_dir_all(&checkout).unwrap();
        fs::write(checkout.join("Cargo.toml"), b"").unwrap();
        for name in ["target", "target-flash-off"] {
            checkout_target(&checkout, name, 1);
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(checkout.join(lease::FILE))
            .unwrap();
        let job = FileLock::try_exclusive(file).unwrap();
        let lane = lease::hold(&checkout.join("target")).expect("the lane claims what it builds");

        let targets = persistent_target_dirs(root.path()).unwrap();

        assert_eq!(
            targets,
            vec![checkout.join("target"), checkout.join("target-flash-off")],
            "a job holding its checkout must not take its caches out of the budget's sight"
        );
        drop((job, lane));
    }

    /// What the ceiling does with the two once it can see them: the cache a lane
    /// is running from survives, and its bytes are still spent, so the idle one
    /// beside it is what pays.
    #[test]
    fn the_cache_a_lane_runs_from_survives_and_the_idle_one_pays() {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("disrupt/kithara");
        fs::create_dir_all(&checkout).unwrap();
        fs::write(checkout.join("Cargo.toml"), b"").unwrap();
        let running = checkout_target(&checkout, "target", 400_000);
        let idle = checkout_target(&checkout, "target-flash-off", 400_000);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(checkout.join(lease::FILE))
            .unwrap();
        let job = FileLock::try_exclusive(file).unwrap();
        let lane = lease::hold(&running).expect("the lane claims what it builds");

        let targets = persistent_target_dirs(root.path()).unwrap();
        enforce_budget(&targets, occupied(&running)).unwrap();

        assert!(
            running.join("debug/artifact").exists(),
            "the cache a lane is executing from must survive the ceiling"
        );
        assert!(
            !idle.join("debug/artifact").exists(),
            "the live cache is charged against the ceiling, so the idle one is evicted"
        );
        drop((job, lane));
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
        let gone = still_there(Err(io::Error::from(io::ErrorKind::NotFound)))
            .expect("a vanished entry is not a failure");
        assert!(gone.is_none(), "a vanished entry is counted as nothing");

        let refused = still_there(Err(io::Error::from(io::ErrorKind::PermissionDenied)));
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
