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

/// The file a process locks for as long as it works in a build directory, so a
/// reclaim running elsewhere can tell "in use" from "left behind".
///
/// A build directory needs protecting for longer than it is being written to.
/// A stress run that had finished compiling and was merely executing its own
/// binaries looked idle, the host's build-cache budget deleted them out from
/// under it, and the repetitions that then failed to exec read as the product
/// breaking rather than the CI eating itself.
pub const FILE: &str = ".kithara-job-lease";

/// The file a held lease rewrites every [`consts::LEASE_HEARTBEAT`], for a
/// reclaim that cannot see the lock.
///
/// A lock does not cross a virtual machine's boundary: a build directory
/// shared into a VM is locked inside it, and a reclaim on the host sees the
/// lock file but not the lock.
/// The last holder to let go removes it, so a directory nobody holds reads as
/// free at once.
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
/// The lock is shared, so several holders coexist: a run and the harness
/// invocations it spawns. A reclaim asks for the same file exclusively, which
/// is the only request a shared holder refuses; a claim waits out a reclaim
/// for up to [`consts::LEASE_WAIT`]. Taking the lease marks the directory used
/// now, which is the date a reclaim orders directories by.
///
/// The result has to be bound: dropping it on the spot releases the claim
/// immediately, which reads at the call site as holding one.
///
/// # Errors
///
/// When the directory or its lease file cannot be made, or a reclaim still
/// holds it at the end of the wait. A job never builds unleased: a reclaim
/// would delete the directory under it.
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
    let current = match Handle::from_path(path) {
        Ok(current) => current,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if Handle::from_file(probe.try_clone()?)? != current {
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
/// holds it now.
///
/// # Errors
///
/// When the lease file cannot be opened or locked for a reason other than a
/// holder.
pub fn evict(directory: &Path) -> io::Result<Option<Eviction>> {
    let file = open(&directory.join(FILE))?;
    match FileLock::try_exclusive(file) {
        Ok(lock) => Ok(Some(Eviction { _lock: lock })),
        Err(fs4::TryLockError::WouldBlock) => Ok(None),
        Err(fs4::TryLockError::Error(error)) => Err(error),
    }
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
    fn drop(&mut self) {
        if let Some(Heartbeat { stop, beating }) = self.heartbeat.take() {
            drop(stop);
            let _ = beating.join();
        }
        drop(self.lock.take());
        // Only a holder that finds nobody else holding the directory may take
        // the heartbeat away; another holder is still beating into it.
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

    use super::{FILE, HEARTBEAT, evict, hold, hold_within, open, take};

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
}
