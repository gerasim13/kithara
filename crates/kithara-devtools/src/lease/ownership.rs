use std::{
    fs::{self, File},
    io,
    path::{self, Path, PathBuf},
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime},
};

use same_file::Handle;
use tracing::warn;

#[cfg(unix)]
pub use super::filesystem::Candidate;
use crate::{
    consts,
    lock::{self, FileLock},
};

/// A confined removal is unavailable on platforms without descriptor-relative operations.
#[cfg(not(unix))]
#[derive(Debug)]
pub struct Candidate;

#[cfg(not(unix))]
impl Candidate {
    /// Opens an owned-root candidate without following links.
    ///
    /// # Errors
    /// Always refuses when confined removal is unsupported.
    pub fn open(_root: &Path, _relative: &Path) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "confined removal requires Unix",
        ))
    }

    /// Metadata of the captured original entry.
    ///
    /// # Errors
    /// Unsupported on this platform.
    pub fn metadata(&self) -> io::Result<fs::Metadata> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "confined metadata requires Unix",
        ))
    }

    /// Opens a captured file without following links.
    ///
    /// # Errors
    /// Unsupported on this platform.
    pub fn open_file(&self, _write: bool) -> io::Result<File> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "confined file access requires Unix",
        ))
    }

    /// Opens a sibling through the captured parent.
    ///
    /// # Errors
    /// Unsupported on this platform.
    pub fn open_sibling(&self, _name: &std::ffi::OsStr) -> io::Result<File> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "confined file access requires Unix",
        ))
    }

    /// Removes the captured candidate, retaining a replacement.
    ///
    /// # Errors
    /// Always refuses when confined removal is unsupported.
    pub fn remove(&self) -> io::Result<bool> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "confined removal requires Unix",
        ))
    }
}

/// Claims one relative resource key under a stable, nonswept lock root.
/// Acquire ancestor keys too when their removal would remove this resource.
///
/// # Errors
/// Invalid keys, unsafe lock files, or a reclaim outlasting the claim bound.
pub fn claim(root: &Path, key: &Path) -> io::Result<FileLock> {
    let path = claim_path(root, key)?;
    FileLock::shared_until(
        open(&path)?,
        "resource ownership",
        Instant::now() + consts::LEASE_WAIT,
    )
}

/// Excludes claims on a resource key until the returned guard drops.
/// Lock files must remain outside swept trees and must never be unlinked.
///
/// # Errors
/// Invalid keys, unsafe lock files, or lock errors other than contention.
pub fn exclude(root: &Path, key: &Path) -> io::Result<Option<FileLock>> {
    try_exclude(open(&claim_path(root, key)?)?)
}

fn claim_path(root: &Path, key: &Path) -> io::Result<PathBuf> {
    if key.as_os_str().is_empty()
        || !key
            .components()
            .all(|part| matches!(part, path::Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ownership key must be a normal relative path",
        ));
    }
    fs::create_dir_all(root)?;
    let key: PathBuf = key.components().collect();
    Ok(root.join(hex::encode(key.as_os_str().as_encoded_bytes())))
}

fn physical(directory: &Path) -> io::Result<PathBuf> {
    let absolute = path::absolute(directory)?;
    let mut existing = absolute.as_path();
    let mut missing = Vec::new();
    loop {
        match fs::canonicalize(existing) {
            Ok(mut resolved) => {
                for name in missing.into_iter().rev() {
                    resolved.push(name);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(
                    existing
                        .file_name()
                        .ok_or_else(|| io::Error::other("build path has no existing ancestor"))?
                        .to_owned(),
                );
                existing = existing
                    .parent()
                    .ok_or_else(|| io::Error::other("build path needs a parent"))?;
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(target_os = "macos")]
fn sidecar(directory: &Path) -> io::Result<PathBuf> {
    let directory = path::absolute(directory)?;
    let parent = directory
        .parent()
        .ok_or_else(|| io::Error::other("leased directory needs a parent"))?;
    let name = directory
        .file_name()
        .ok_or_else(|| io::Error::other("leased directory needs a name"))?;
    let mut sidecar = std::ffi::OsString::from(".");
    sidecar.push(name);
    sidecar.push(FILE);
    fs::create_dir_all(parent)?;
    Ok(parent.join(sidecar))
}

fn try_exclude(file: File) -> io::Result<Option<FileLock>> {
    match FileLock::try_exclusive(file) {
        Ok(lock) => Ok(Some(lock)),
        Err(fs4::TryLockError::WouldBlock) => Ok(None),
        Err(fs4::TryLockError::Error(error)) => Err(error),
    }
}

/// Lease filename protecting a build directory during compilation and execution.
pub const FILE: &str = ".kithara-job-lease";

/// Heartbeat filename refreshed every [`consts::LEASE_HEARTBEAT`] for reclaims
/// outside the lock's virtual machine. The last holder removes the heartbeat.
pub const HEARTBEAT: &str = ".kithara-job-heartbeat";

/// A live claim on a build directory, released when this drops or the process
/// dies.
#[derive(Debug)]
pub struct Lease {
    directory: PathBuf,
    heartbeat: Option<Heartbeat>,
    lock: Option<FileLock>,
    probe: File,
    _stable: Option<FileLock>,
    _ancestors: Vec<FileLock>,
}

/// A reclaim's hold on a build directory: no lease can be taken while it lives.
#[derive(Debug)]
pub struct Eviction {
    _lock: FileLock,
    _stable: Option<FileLock>,
}

#[derive(Debug)]
struct Heartbeat {
    stop: Sender<()>,
    beating: JoinHandle<()>,
}

/// Claims `directory` until the returned guard drops, creating it first.
///
/// Shared holders coexist with spawned harness runs; only an exclusive reclaim
/// blocks a claim. Claims wait up to [`consts::LEASE_WAIT`] and mark the directory
/// used now, which reclaim uses to order candidates.
/// Bind the guard; dropping it immediately releases the claim.
///
/// # Errors
/// Directory/lease creation fails, or a reclaim outlasts the wait. Never build
/// without a lease: reclaim could delete the live directory.
pub fn hold(directory: &Path) -> io::Result<Lease> {
    hold_within(directory, consts::LEASE_WAIT)
}

/// Claims a relative build path and its removable ancestors before creating them.
/// `root` itself must not be reclaimed while this scope is in use.
///
/// # Errors
/// Non-normal keys, creation failures, or an eviction outlasting the wait.
#[cfg(target_os = "macos")]
pub fn hold_in(root: &Path, key: &Path) -> io::Result<Lease> {
    if key.as_os_str().is_empty()
        || !key
            .components()
            .all(|part| matches!(part, path::Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "build key must be a normal relative path",
        ));
    }
    let deadline = Instant::now() + consts::LEASE_WAIT;
    let root = physical(root)?;
    let lock_root = root
        .parent()
        .ok_or_else(|| io::Error::other("build scope needs a parent"))?
        .join(".kithara-job-leases");
    let mut scoped_key = PathBuf::from(
        root.file_name()
            .ok_or_else(|| io::Error::other("build scope needs a name"))?,
    );
    let mut directory = root.to_path_buf();
    let mut ancestors = Vec::new();
    let mut parts = key.components().peekable();
    while let Some(part) = parts.next() {
        directory.push(part.as_os_str());
        scoped_key.push(part.as_os_str());
        ancestors.push(claim(&lock_root, &scoped_key)?);
        if parts.peek().is_some() {
            ancestors.push(FileLock::shared_until(
                open(&sidecar(&directory)?)?,
                "build ancestor ownership",
                deadline,
            )?);
            fs::create_dir_all(&directory)?;
        }
    }
    let mut lease = hold_within(
        &directory,
        deadline.saturating_duration_since(Instant::now()),
    )?;
    lease._ancestors = ancestors;
    Ok(lease)
}

fn hold_within(directory: &Path, bound: Duration) -> io::Result<Lease> {
    let directory = physical(directory)?;
    let directory = directory.as_path();
    let deadline = Instant::now() + bound;
    #[cfg(target_os = "macos")]
    let stable = Some(FileLock::shared_until(
        open(&sidecar(directory)?)?,
        "build ownership",
        deadline,
    )?);
    #[cfg(not(target_os = "macos"))]
    let stable = None;
    loop {
        fs::create_dir_all(directory)?;
        let path = path::absolute(directory)?.join(FILE);
        if let Some(mut lease) = take(open(&path)?, &path, deadline)? {
            lease._stable = stable;
            return Ok(lease);
        }
    }
}

/// Takes the shared lock on the open lease `file` at `path`.
///
/// A reclaim moves a directory away before it lets go of the lock, so a lock
/// a waiter then gets can guard a file that no longer sits at `path`. That
/// lock is no lease, and `None` tells the caller to open the lease again.
fn take(file: File, path: &Path, deadline: Instant) -> io::Result<Option<Lease>> {
    let probe = file.try_clone()?;
    let subject = format!("the build directory lease {}", path.display());
    let lock = FileLock::shared_until(file, &subject, deadline)?;
    if !still_at(&probe, path)? {
        return Ok(None);
    }
    probe.set_modified(SystemTime::now())?;
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other(format!("{} names no directory", path.display())))?
        .to_path_buf();
    Ok(Some(Lease {
        heartbeat: Some(Heartbeat::start(directory.join(HEARTBEAT))?),
        directory,
        lock: Some(lock),
        probe,
        _stable: None,
        _ancestors: Vec::new(),
    }))
}

/// Takes `directory`'s lease exclusively, so no job can claim it, unless a job
/// holds it now or another eviction moved it away meanwhile.
///
/// # Errors
///
/// When the lease file cannot be opened or locked for a reason other than a
/// holder.
pub fn evict(directory: &Path) -> io::Result<Option<Eviction>> {
    #[cfg(target_os = "macos")]
    let stable = match try_exclude(open(&sidecar(directory)?)?)? {
        Some(stable) => Some(stable),
        None => return Ok(None),
    };
    #[cfg(not(target_os = "macos"))]
    let stable = None;
    let path = directory.join(FILE);
    Ok(fence(open(&path)?, &path)?.map(|mut eviction| {
        eviction._stable = stable;
        eviction
    }))
}

/// Takes the exclusive lock on the open lease `file` at `path`, or nothing
/// when a job holds it or the lease no longer sits at `path`: another eviction
/// moved the directory away while this one opened it, and the lock would
/// guard the moved directory, not whatever stands at `path` now.
fn fence(file: File, path: &Path) -> io::Result<Option<Eviction>> {
    let probe = file.try_clone()?;
    let lock = match FileLock::try_exclusive(file) {
        Ok(lock) => lock,
        Err(fs4::TryLockError::WouldBlock) => return Ok(None),
        Err(fs4::TryLockError::Error(error)) => return Err(error),
    };
    Ok(still_at(&probe, path)?.then_some(Eviction {
        _lock: lock,
        _stable: None,
    }))
}

/// Whether the open lease `file` is still the one at `path`.
fn still_at(file: &File, path: &Path) -> io::Result<bool> {
    let current = match Handle::from_path(path) {
        Ok(current) => current,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    Ok(Handle::from_file(file.try_clone()?)? == current)
}

fn open(path: &Path) -> io::Result<File> {
    lock::open(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("opening lease file {}: {error}", path.display()),
        )
    })
}

impl Heartbeat {
    fn start(path: PathBuf) -> io::Result<Self> {
        beat(&path)?;
        let (stop, stopped) = mpsc::channel::<()>();
        let beating = thread::Builder::new()
            .name("lease-heartbeat".to_owned())
            .spawn(move || {
                while let Err(RecvTimeoutError::Timeout) =
                    stopped.recv_timeout(consts::LEASE_HEARTBEAT)
                {
                    if let Err(error) = beat(&path) {
                        warn!("lease heartbeat {} not refreshed: {error}", path.display());
                    }
                }
            })?;
        Ok(Self { stop, beating })
    }
}

fn beat(path: &Path) -> io::Result<()> {
    open(path)?.set_modified(SystemTime::now())
}

impl Drop for Lease {
    /// Stops this holder's heartbeat before releasing its shared lock. Only the
    /// last holder records completion and removes the heartbeat file, under an
    /// exclusive eviction lock.
    fn drop(&mut self) {
        if let Some(Heartbeat { stop, beating }) = self.heartbeat.take() {
            drop(stop);
            let _ = beating.join();
        }
        drop(self.lock.take());
        let lease = self.directory.join(FILE);
        let fenced = self.probe.try_clone().and_then(|file| fence(file, &lease));
        if let Ok(Some(_last)) = fenced {
            if let Err(error) = self.probe.set_modified(SystemTime::now()) {
                warn!(path = %lease.display(), %error, "lease completion not recorded");
            }
            let _ = fs::remove_file(self.directory.join(HEARTBEAT));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs, io, thread,
        time::{Duration, Instant, UNIX_EPOCH},
    };

    use tempfile::TempDir;

    use super::{FILE, HEARTBEAT, evict, fence, hold, hold_within, open, take};

    #[test]
    fn a_lease_names_a_directory_that_did_not_exist_yet() {
        let temp = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let build = temp.path().join("target-stress");

        let lease = hold(&build).expect("claim a build directory before it is built into");

        assert!(build.join(FILE).is_file());
        drop(lease);
    }

    #[test]
    fn two_holders_of_one_directory_coexist() {
        let temp = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();

        let first = hold(temp.path()).expect("first holder");
        let second = hold(temp.path()).expect("a spawned harness run must not be refused");

        drop((first, second));
    }

    /// A lock does not cross a virtual machine's boundary, so a lease also
    /// says it is alive in a file an evictor on the other side can read.
    #[test]
    fn a_held_lease_beats() {
        let temp = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();

        let lease = hold(temp.path()).unwrap();

        assert!(temp.path().join(HEARTBEAT).is_file());
        drop(lease);
    }

    #[test]
    fn the_last_holder_takes_the_heartbeat_with_it() {
        let temp = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let first = hold(temp.path()).unwrap();
        let second = hold(temp.path()).unwrap();
        let lease_file = open(&temp.path().join(FILE)).unwrap();
        let started = UNIX_EPOCH + Duration::from_secs(10);
        lease_file.set_modified(started).unwrap();

        drop(first);
        assert!(
            temp.path().join(HEARTBEAT).is_file(),
            "a holder still beats into the directory"
        );
        assert_eq!(lease_file.metadata().unwrap().modified().unwrap(), started);
        assert!(evict(temp.path()).unwrap().is_none());
        drop(second);

        assert!(
            !temp.path().join(HEARTBEAT).exists(),
            "a directory nobody holds reads as free at once, not after the heartbeat goes stale"
        );
        assert!(lease_file.metadata().unwrap().modified().unwrap() > started);
        assert!(evict(temp.path()).unwrap().is_some());
    }

    #[test]
    fn an_eviction_is_refused_while_a_lease_is_held() {
        let temp = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let lease = hold(temp.path()).unwrap();

        assert!(evict(temp.path()).unwrap().is_none());
        drop(lease);
        assert!(evict(temp.path()).unwrap().is_some());
    }

    #[test]
    fn a_released_holder_does_not_touch_a_replacement_directory() {
        let temp = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let build = temp.path().join("lane");
        let mut lease = hold(&build).unwrap();
        let heartbeat = lease.heartbeat.take().unwrap();
        drop(heartbeat.stop);
        heartbeat.beating.join().unwrap();
        drop(lease.lock.take());
        drop(lease._stable.take());

        let eviction = evict(&build).unwrap().unwrap();
        fs::rename(&build, temp.path().join(".evicting-lane")).unwrap();
        fs::create_dir_all(&build).unwrap();
        let marker = open(&build.join(FILE)).unwrap();
        let heartbeat = open(&build.join(HEARTBEAT)).unwrap();
        let modified = UNIX_EPOCH + Duration::from_secs(10);
        marker.set_modified(modified).unwrap();
        heartbeat.set_modified(modified).unwrap();
        drop(eviction);

        drop(lease);

        assert_eq!(marker.metadata().unwrap().modified().unwrap(), modified);
        assert!(build.join(HEARTBEAT).is_file());
        assert_eq!(heartbeat.metadata().unwrap().modified().unwrap(), modified);
    }

    #[test]
    fn a_lease_waits_out_an_eviction() {
        let temp = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let eviction = evict(temp.path())
            .unwrap()
            .expect("nobody leases the directory");
        let evictor = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            drop(eviction);
        });

        let lease = hold(temp.path());

        evictor.join().unwrap();
        lease.expect("a job waits for an eviction to let go instead of failing");
    }

    #[test]
    fn a_lease_not_taken_within_its_bound_fails() {
        let temp = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let eviction = evict(temp.path())
            .unwrap()
            .expect("nobody leases the directory");

        let error = hold_within(temp.path(), Duration::from_millis(50))
            .expect_err("a job never builds unleased");

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        drop(eviction);
    }

    /// An evictor moves a directory away before it lets go of its lease, so
    /// the lock a waiting job then gets guards a directory that is gone.
    #[test]
    fn a_lock_on_a_lease_moved_away_is_no_lease() {
        let temp = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let build = temp.path().join("lane");
        fs::create_dir_all(&build).unwrap();
        let path = build.join(FILE);
        let waiting = open(&path).unwrap();
        fs::rename(&build, temp.path().join(".evicting-lane")).unwrap();
        fs::create_dir_all(&build).unwrap();

        let taken = take(waiting, &path, Instant::now()).unwrap();

        assert!(taken.is_none(), "a lock on the moved lease guards nothing");
    }

    /// Two evictions race for one directory: the one that loses opens the
    /// lease, the winner moves the directory aside, and a job builds in a new
    /// one at the old path. The loser's lock guards the moved directory, and
    /// taking it for the new one would move a live build away.
    #[test]
    fn an_eviction_lock_on_a_lease_moved_away_is_no_eviction() {
        let temp = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let build = temp.path().join("lane");
        fs::create_dir_all(&build).unwrap();
        let path = build.join(FILE);
        let losing = open(&path).unwrap();
        fs::rename(&build, temp.path().join(".evicting-lane")).unwrap();
        fs::create_dir_all(&build).unwrap();

        let fenced = fence(losing, &path).unwrap();

        assert!(fenced.is_none(), "a lock on the moved lease fences nothing");
    }
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_mounted_target_needs_no_writable_sibling_lease() {
        let root = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let target = root.path().join("target");
        fs::create_dir(&target).unwrap();
        let sibling = root.path().join(".target.kithara-job-lease");
        fs::create_dir(&sibling).unwrap();
        let held = hold(&target).unwrap();
        assert!(evict(&target).unwrap().is_none());
        drop(held);
        assert!(evict(&target).unwrap().is_some());
        assert!(sibling.is_dir());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn eviction_fences_creation_and_survives_replacement() {
        let temp = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let build = temp.path().join("not-created");
        let stable = super::try_exclude(open(&super::sidecar(&build).unwrap()).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            hold_within(&build, Duration::ZERO).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(!build.exists());
        fs::create_dir(&build).unwrap();
        fs::rename(&build, temp.path().join("moved")).unwrap();
        fs::create_dir(&build).unwrap();
        assert_eq!(
            hold_within(&build, Duration::ZERO).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        drop(stable);
        assert!(hold(&build).is_ok());
    }

    #[test]
    fn namespace_ownership_outlives_recency_and_does_not_block_peers() {
        let root = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let key = std::path::Path::new("cache/review");
        let first = super::claim(root.path(), key).unwrap();
        let second = super::claim(root.path(), key).unwrap();
        open(&super::claim_path(root.path(), key).unwrap())
            .unwrap()
            .set_modified(UNIX_EPOCH)
            .unwrap();
        assert!(super::exclude(root.path(), key).unwrap().is_none());
        assert!(
            super::exclude(root.path(), std::path::Path::new("cache/trusted"))
                .unwrap()
                .is_some()
        );
        drop(first);
        assert!(super::exclude(root.path(), key).unwrap().is_none());
        drop(second);
        assert!(super::exclude(root.path(), key).unwrap().is_some());
        assert!(super::claim(root.path(), std::path::Path::new("../external")).is_err());
        let normalized = super::claim(root.path(), std::path::Path::new("a/b")).unwrap();
        assert!(
            super::exclude(root.path(), std::path::Path::new("a//./b"))
                .unwrap()
                .is_none()
        );
        drop(normalized);
    }

    #[cfg(unix)]
    #[test]
    fn confined_removal_retains_replaced_candidates_and_parents() {
        let root = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let original = root.path().join("parent/entry");
        fs::create_dir_all(&original).unwrap();
        fs::write(original.join("sentinel"), b"original").unwrap();
        let candidate =
            super::Candidate::open(root.path(), std::path::Path::new("parent/entry")).unwrap();
        fs::rename(&original, root.path().join("old")).unwrap();
        fs::create_dir(&original).unwrap();
        fs::write(original.join("sentinel"), b"replacement").unwrap();
        assert!(!candidate.remove().unwrap());
        assert_eq!(fs::read(original.join("sentinel")).unwrap(), b"replacement");
        let candidate =
            super::Candidate::open(root.path(), std::path::Path::new("parent/entry")).unwrap();
        fs::rename(root.path().join("parent"), root.path().join("moved")).unwrap();
        fs::create_dir_all(&original).unwrap();
        assert!(!candidate.remove().unwrap());
        assert!(original.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn confined_removal_never_follows_root_or_intermediate_or_child_links() {
        use std::os::unix::fs::symlink;
        let root = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let external = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        fs::write(external.path().join("sentinel"), b"keep").unwrap();
        symlink(external.path(), root.path().join("link")).unwrap();
        assert!(
            super::Candidate::open(root.path(), std::path::Path::new("link/sentinel")).is_err()
        );
        assert!(
            super::Candidate::open(&root.path().join("link"), std::path::Path::new("sentinel"))
                .is_err()
        );
        fs::create_dir(root.path().join("entry")).unwrap();
        symlink(external.path(), root.path().join("entry/link")).unwrap();
        let candidate = super::Candidate::open(root.path(), std::path::Path::new("entry")).unwrap();
        assert!(candidate.remove().unwrap());
        assert_eq!(fs::read(external.path().join("sentinel")).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn file_access_and_metadata_stay_with_the_inspected_parent() {
        let root = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        fs::create_dir(root.path().join("logs")).unwrap();
        fs::write(root.path().join("logs/current"), b"original").unwrap();
        let candidate =
            super::Candidate::open(root.path(), std::path::Path::new("logs/current")).unwrap();
        fs::rename(root.path().join("logs"), root.path().join("old")).unwrap();
        fs::create_dir(root.path().join("logs")).unwrap();
        fs::write(root.path().join("logs/current"), b"replacement").unwrap();
        assert_eq!(candidate.metadata().unwrap().len(), 8);
        candidate.open_file(true).unwrap().set_len(0).unwrap();
        candidate
            .open_sibling(std::ffi::OsStr::new("current.1"))
            .unwrap()
            .set_len(4)
            .unwrap();
        assert_eq!(
            fs::read(root.path().join("logs/current")).unwrap(),
            b"replacement"
        );
        assert!(root.path().join("old/current.1").exists());
        assert!(!root.path().join("logs/current.1").exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn scoped_ownership_prevents_ancestor_eviction_after_enumeration() {
        let root = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let slot = root.path().join("slot");
        let running = super::hold_in(root.path(), std::path::Path::new("slot/cargo")).unwrap();
        assert!(evict(&slot).unwrap().is_none());
        drop(running);
        let removing = evict(&slot).unwrap().unwrap();
        assert_eq!(
            hold_within(&slot, Duration::ZERO).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        drop(removing);
        assert!(evict(&slot).unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn interrupted_recovery_keeps_a_replacement_on_every_pass() {
        let root = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let aside = root.path().join(".evicting-interrupted");
        fs::create_dir_all(aside.join("data")).unwrap();
        fs::write(aside.join("data/sentinel"), b"replacement").unwrap();
        fs::write(aside.join(FILE), b"eviction=0:0\n").unwrap();
        let identity = super::Handle::from_path(&aside).unwrap();
        let entries = fs::read_dir(root.path()).unwrap().count();
        for _ in 0..3 {
            let candidate =
                super::Candidate::open(root.path(), std::path::Path::new(".evicting-interrupted"))
                    .unwrap();
            assert!(!candidate.remove().unwrap());
            assert_eq!(super::Handle::from_path(&aside).unwrap(), identity);
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), entries);
            assert_eq!(
                fs::read(aside.join("data/sentinel")).unwrap(),
                b"replacement"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn namespace_removal_retains_an_active_nested_eviction() {
        let root = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let namespace = root.path().join("namespace");
        let nested = namespace.join(".evicting-inner");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("sentinel"), b"active").unwrap();
        let active = evict(&nested).unwrap().unwrap();
        let candidate =
            super::Candidate::open(root.path(), std::path::Path::new("namespace")).unwrap();
        assert_eq!(
            candidate.remove().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        let aside = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.is_dir())
            .unwrap();
        assert_eq!(
            fs::read(aside.join("data/.evicting-inner/sentinel")).unwrap(),
            b"active"
        );
        drop(active);
        let candidate = super::Candidate::open(
            root.path(),
            std::path::Path::new(aside.file_name().unwrap()),
        )
        .unwrap();
        assert!(candidate.remove().unwrap());
    }

    #[test]
    fn configured_checkout_relative_targets_claim_the_physical_directory() {
        let root = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let checkout = root.path().join("project");
        fs::create_dir(&checkout).unwrap();
        let target = checkout.join("../.target/xtask");
        let held = hold(&target).unwrap();
        let physical = root.path().join(".target/xtask");
        assert!(physical.join(FILE).is_file());
        assert!(evict(&physical).unwrap().is_none());
        drop(held);
        assert!(evict(&physical).unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn writable_handles_refuse_hardlinks_and_unlink_preserves_the_external_file() {
        for operation in ["lease", "file", "sibling", "unlink"] {
            let root = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
            let external = TempDir::new_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
            let sentinel = external.path().join("sentinel");
            fs::write(&sentinel, b"external").unwrap();
            let key = std::path::Path::new("cache/review");
            let link = if operation == "lease" {
                super::claim_path(root.path(), key).unwrap()
            } else {
                root.path().join("linked")
            };
            fs::hard_link(&sentinel, &link).unwrap();
            match operation {
                "lease" => assert!(super::claim(root.path(), key).is_err()),
                "file" => {
                    let candidate =
                        super::Candidate::open(root.path(), std::path::Path::new("linked"))
                            .unwrap();
                    assert!(candidate.open_file(true).is_err());
                }
                "sibling" => {
                    fs::write(root.path().join("current"), b"log").unwrap();
                    let candidate =
                        super::Candidate::open(root.path(), std::path::Path::new("current"))
                            .unwrap();
                    assert!(
                        candidate
                            .open_sibling(std::ffi::OsStr::new("linked"))
                            .is_err()
                    );
                }
                "unlink" => assert!(
                    super::Candidate::open(root.path(), std::path::Path::new("linked"))
                        .unwrap()
                        .remove()
                        .unwrap()
                ),
                _ => unreachable!(),
            }
            assert_eq!(fs::read(&sentinel).unwrap(), b"external", "{operation}");
        }
    }
}
