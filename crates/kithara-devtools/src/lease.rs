use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{self, Path, PathBuf},
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime},
};

use same_file::Handle;
use tracing::warn;

use crate::{consts, lock::FileLock};

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
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
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
    /// last holder removes the heartbeat file, under an exclusive eviction lock.
    fn drop(&mut self) {
        if let Some(Heartbeat { stop, beating }) = self.heartbeat.take() {
            drop(stop);
            let _ = beating.join();
        }
        drop(self.lock.take());
        if let Ok(Some(_last)) = evict(&self.directory) {
            let _ = fs::remove_file(self.directory.join(HEARTBEAT));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs, io, thread,
        time::{Duration, Instant},
    };

    use tempfile::TempDir;

    use super::{FILE, HEARTBEAT, evict, fence, hold, hold_within, open, take};

    #[test]
    fn a_lease_names_a_directory_that_did_not_exist_yet() {
        let temp = TempDir::new().unwrap();
        let build = temp.path().join("target-stress");

        let lease = hold(&build).expect("claim a build directory before it is built into");

        assert!(build.join(FILE).is_file());
        drop(lease);
    }

    #[test]
    fn two_holders_of_one_directory_coexist() {
        let temp = TempDir::new().unwrap();

        let first = hold(temp.path()).expect("first holder");
        let second = hold(temp.path()).expect("a spawned harness run must not be refused");

        drop((first, second));
    }

    /// A lock does not cross a virtual machine's boundary, so a lease also
    /// says it is alive in a file an evictor on the other side can read.
    #[test]
    fn a_held_lease_beats() {
        let temp = TempDir::new().unwrap();

        let lease = hold(temp.path()).unwrap();

        assert!(temp.path().join(HEARTBEAT).is_file());
        drop(lease);
    }

    #[test]
    fn the_last_holder_takes_the_heartbeat_with_it() {
        let temp = TempDir::new().unwrap();
        let first = hold(temp.path()).unwrap();
        let second = hold(temp.path()).unwrap();

        drop(first);
        assert!(
            temp.path().join(HEARTBEAT).is_file(),
            "a holder still beats into the directory"
        );
        drop(second);

        assert!(
            !temp.path().join(HEARTBEAT).exists(),
            "a directory nobody holds reads as free at once, not after the heartbeat goes stale"
        );
    }

    #[test]
    fn an_eviction_is_refused_while_a_lease_is_held() {
        let temp = TempDir::new().unwrap();
        let lease = hold(temp.path()).unwrap();

        assert!(evict(temp.path()).unwrap().is_none());
        drop(lease);
        assert!(evict(temp.path()).unwrap().is_some());
    }

    #[test]
    fn a_lease_waits_out_an_eviction() {
        let temp = TempDir::new().unwrap();
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
        let temp = TempDir::new().unwrap();
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
        let temp = TempDir::new().unwrap();
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
        let temp = TempDir::new().unwrap();
        let build = temp.path().join("lane");
        fs::create_dir_all(&build).unwrap();
        let path = build.join(FILE);
        let losing = open(&path).unwrap();
        fs::rename(&build, temp.path().join(".evicting-lane")).unwrap();
        fs::create_dir_all(&build).unwrap();

        let fenced = fence(losing, &path).unwrap();

        assert!(fenced.is_none(), "a lock on the moved lease fences nothing");
    }
}
