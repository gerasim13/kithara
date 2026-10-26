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
}

/// A reclaim's hold on a build directory: no lease can be taken while it lives.
#[derive(Debug)]
pub struct Eviction {
    _lock: FileLock,
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

fn hold_within(directory: &Path, bound: Duration) -> io::Result<Lease> {
    let directory = physical(directory)?;
    let directory = directory.as_path();
    let deadline = Instant::now() + bound;
    loop {
        fs::create_dir_all(directory)?;
        let path = path::absolute(directory)?.join(FILE);
        if let Some(lease) = take(open(&path)?, &path, deadline)? {
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
    let path = directory.join(FILE);
    fence(open(&path)?, &path)
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
    Ok(still_at(&probe, path)?.then_some(Eviction { _lock: lock }))
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
