use std::{
    env,
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt as _},
    ffi::OsString,
    path::{Path, PathBuf},
    process::{self, Command, ExitStatus},
    thread,
    time::{Duration, Instant, SystemTime},
};

mod consts {
    use super::Duration;

    pub(super) const LEASE_FILE: &str = ".kithara-job-lease";
    pub(super) const HEARTBEAT_FILE: &str = ".kithara-job-heartbeat";
    // Refresh far faster than the five-minute host cleanup interval. Polling the
    // child at 100 ms keeps the wrapper's exit latency below a measurable CI phase.
    pub(super) const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
    pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(100);
    // A job waits on the lease only while the host's cache cleanup holds it,
    // which ends within one eviction pass.
    pub(super) const LOCK_POLL: Duration = Duration::from_secs(1);
    pub(super) const LOCK_ANNOUNCE: Duration = Duration::from_secs(30);
}

struct Heartbeat {
    path: PathBuf,
    lease: PathBuf,
    file: File,
}

impl Heartbeat {
    fn start(file: &File, lease: &Path) -> io::Result<Self> {
        let parent = lease
            .parent()
            .ok_or_else(|| io::Error::other("lease path has no build directory"))?;
        let heartbeat = Self {
            path: parent.join(consts::HEARTBEAT_FILE),
            lease: lease.to_path_buf(),
            file: file.try_clone().map_err(|error| context(error, "cloning lease for heartbeat", lease))?,
        };
        heartbeat.refresh()?;
        Ok(heartbeat)
    }

    fn refresh(&self) -> io::Result<()> {
        open_lock(&self.path)?.set_modified(SystemTime::now())
            .map_err(|error| context(error, "refreshing heartbeat", &self.path))
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        if self.file.unlock().is_ok() && self.file.try_lock().is_ok() {
            if still_at(&self.file, &self.lease).unwrap_or(false) {
                let _ = self.file.set_modified(SystemTime::now());
                let _ = fs::remove_file(&self.path);
            }
            let _ = self.file.unlock();
        }
    }
}

struct Shared(File);

impl Drop for Shared {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn context(error: io::Error, operation: &str, path: &Path) -> io::Error {
    io::Error::new(error.kind(), format!("{operation} {}: {error}", path.display()))
}

fn open_lock(path: &Path) -> io::Result<File> {
    #[cfg(target_os = "macos")]
    const NOFOLLOW: i32 = 0x100;
    #[cfg(not(target_os = "macos"))]
    const NOFOLLOW: i32 = 0x20000;
    let mut options = OpenOptions::new();
    options.read(true).write(true).custom_flags(NOFOLLOW);
    let file = match options.create_new(true).open(path) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => options.create_new(false).open(path),
        result => result,
    }.map_err(|error| context(error, "opening lease or heartbeat", path))?;
    let metadata = file.metadata().map_err(|error| context(error, "inspecting lease or heartbeat", path))?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} must be a singly linked regular file", path.display())));
    }
    Ok(file)
}

fn claim(path: &Path) -> io::Result<Shared> {
    fs::create_dir_all(path.parent().ok_or_else(|| io::Error::other("lease needs a parent"))?)
        .map_err(|error| context(error, "creating lease parent", path))?;
    let file = open_lock(path)?;
    hold_shared(&file, path)?;
    Ok(Shared(file))
}

fn physical(directory: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(directory)?;
    let mut existing = absolute.as_path();
    let mut missing = Vec::new();
    loop {
        match fs::canonicalize(existing) {
            Ok(mut resolved) => {
                for name in missing.into_iter().rev() { resolved.push(name); }
                return Ok(resolved);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(existing.file_name().ok_or_else(|| io::Error::other("build path has no existing ancestor"))?.to_owned());
                existing = existing.parent().ok_or_else(|| io::Error::other("build path needs a parent"))?;
            }
            Err(error) => return Err(error),
        }
    }
}

fn sidecar(directory: &Path) -> io::Result<PathBuf> {
    let directory = std::path::absolute(directory)?;
    let name = directory.file_name().ok_or_else(|| io::Error::other("build needs a name"))?;
    let mut sidecar = OsString::from(".");
    sidecar.push(name);
    sidecar.push(consts::LEASE_FILE);
    Ok(directory.parent().ok_or_else(|| io::Error::other("build needs a parent"))?.join(sidecar))
}

fn namespace_claims(directory: &Path) -> io::Result<Vec<Shared>> {
    let Some(tree) = env::var_os("KITHARA_CI_CACHE_ROOT").map(PathBuf::from) else {
        return Ok(Vec::new());
    };
    let tree = physical(&tree).map_err(|error| context(error, "resolving cache tree", &tree))?;
    let Ok(relative) = directory.strip_prefix(&tree) else {
        return Ok(Vec::new());
    };
    let root = tree.parent().ok_or_else(|| io::Error::other("cache needs a parent"))?.join(".kithara-job-leases");
    let mut key = PathBuf::from(tree.file_name().ok_or_else(|| io::Error::other("cache needs a name"))?);
    let mut claims = Vec::new();
    let mut protected = tree.clone();
    for part in relative.components() {
        let std::path::Component::Normal(name) = part else {
            return Err(io::Error::other("cache target must have normal relative components"));
        };
        key.push(name);
        let encoded: String = key.as_os_str().as_encoded_bytes().iter().map(|byte| format!("{byte:02x}")).collect();
        claims.push(claim(&root.join(encoded))?);
        protected.push(name);
        claims.push(claim(&sidecar(&protected)?)?);
        fs::create_dir_all(&protected).map_err(|error| context(error, "creating protected cache directory", &protected))?;
    }
    Ok(claims)
}

/// Takes the lease's shared lock. Only the host's cache cleanup takes it
/// exclusively, so a wait names it; this helper has no logger, and stderr is
/// the job log.
fn hold_shared(file: &fs::File, lease: &Path) -> io::Result<()> {
    let started = Instant::now();
    let mut announced: Option<Instant> = None;
    loop {
        match file.try_lock_shared() {
            Ok(()) => break,
            Err(fs::TryLockError::WouldBlock) => {}
            Err(fs::TryLockError::Error(error)) => return Err(context(error, "acquiring shared lease", lease)),
        }
        if announced.is_none_or(|at| at.elapsed() >= consts::LOCK_ANNOUNCE) {
            eprintln!(
                "waiting for the CI build target lease {} ({} s so far): a cache cleanup holds it",
                lease.display(),
                started.elapsed().as_secs()
            );
            announced = Some(Instant::now());
        }
        thread::sleep(consts::LOCK_POLL);
    }
    if announced.is_some() {
        eprintln!(
            "took the CI build target lease {} after {} s",
            lease.display(),
            started.elapsed().as_secs()
        );
    }
    Ok(())
}

/// Holds the build directory's lease, marked used now: the date an eviction
/// orders directories by.
///
/// An eviction moves a directory away before it lets go of the lease, so a
/// lock this job then gets can guard a file that no longer sits at the path.
/// That lock holds nothing, and the lease of whatever stands there now is
/// taken instead.
fn take(directory: &Path, lease: &Path) -> io::Result<File> {
    loop {
        fs::create_dir_all(directory).map_err(|error| context(error, "creating leased build directory", directory))?;
        let file = open_lock(lease)?;
        hold_shared(&file, lease)?;
        if still_at(&file, lease)? {
            file.set_modified(SystemTime::now()).map_err(|error| context(error, "recording lease use", lease))?;
            return Ok(file);
        }
    }
}

/// Whether the open `file` is still the one at `path`.
fn still_at(file: &File, path: &Path) -> io::Result<bool> {
    let held = file.metadata().map_err(|error| context(error, "inspecting open lease", path))?;
    match fs::metadata(path) {
        Ok(current) => Ok(current.dev() == held.dev() && current.ino() == held.ino()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(context(error, "rechecking lease identity", path)),
    }
}

fn main() {
    match run() {
        Ok(status) => process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("failed to hold the CI build target: {error}");
            process::exit(1);
        }
    }
}

fn run() -> io::Result<ExitStatus> {
    let mut args = env::args_os().skip(1);
    let directory = PathBuf::from(
        args.next()
            .ok_or_else(|| io::Error::other("missing build directory"))?,
    );
    let command = args
        .next()
        .ok_or_else(|| io::Error::other("missing command"))?;
    let directory = physical(&directory).map_err(|error| context(error, "resolving build directory", &directory))?;
    let _namespace = namespace_claims(&directory)?;
    let _stable = claim(&sidecar(&directory)?)?;
    let lease = directory.join(consts::LEASE_FILE);
    let _file = take(&directory, &lease)?;
    let heartbeat = Heartbeat::start(&_file, &lease)?;
    let _ = env::current_exe().and_then(fs::remove_file);
    let mut child = Command::new(&command).args(args).spawn()
        .map_err(|error| context(error, "starting leased command", Path::new(&command)))?;
    let mut refresh_at = Instant::now() + consts::HEARTBEAT_INTERVAL;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(context(error, "waiting for leased command", &directory));
            }
        }
        if Instant::now() >= refresh_at {
            if let Err(error) = heartbeat.refresh() {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
            refresh_at = Instant::now() + consts::HEARTBEAT_INTERVAL;
        }
        thread::sleep(consts::POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simultaneous_first_opens_preserve_one_lock_identity() {
        use std::sync::{Arc, Barrier};
        let root = env::temp_dir().join(format!("kithara-bootstrap-first-open-{}", process::id()));
        fs::create_dir(&root).unwrap();
        let path = root.join("lock");
        let barrier = Arc::new(Barrier::new(2));
        let workers = (0..2).map(|_| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                open_lock(&path)
            })
        }).collect::<Vec<_>>();
        let results = workers.into_iter().map(|worker| worker.join().unwrap()).collect::<Vec<_>>();
        let files = results.into_iter().map(Result::unwrap).collect::<Vec<_>>();
        assert_eq!(files[0].metadata().unwrap().ino(), files[1].metadata().unwrap().ino());
        files[0].try_lock().unwrap();
        assert!(matches!(files[1].try_lock(), Err(fs::TryLockError::WouldBlock)));
        files[0].unlock().unwrap();
        fs::write(&path, b"retained").unwrap();
        drop(open_lock(&path).unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"retained");
        drop(files);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn startup_sidecar_precedes_creation_and_matches_shared_ownership() {
        let root = env::temp_dir().join(format!("kithara-bootstrap-{}", process::id()));
        let build = root.join("target");
        fs::create_dir_all(&root).unwrap();
        let held = claim(&sidecar(&build).unwrap()).unwrap();
        assert!(!build.exists());
        let contender = open_lock(&root.join(".target.kithara-job-lease")).unwrap();
        assert!(matches!(contender.try_lock(), Err(fs::TryLockError::WouldBlock)));
        drop(held);
        assert!(contender.try_lock().is_ok());
        contender.unlock().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn heartbeat_survives_until_the_last_shared_holder_finishes() {
        let root = env::temp_dir().join(format!("kithara-bootstrap-heartbeat-{}", process::id()));
        let path = root.join(consts::LEASE_FILE);
        let first = take(&root, &path).unwrap();
        let first_heartbeat = Heartbeat::start(&first, &path).unwrap();
        let second = take(&root, &path).unwrap();
        let second_heartbeat = Heartbeat::start(&second, &path).unwrap();
        drop(first_heartbeat);
        assert!(root.join(consts::HEARTBEAT_FILE).exists());
        drop(second_heartbeat);
        assert!(!root.join(consts::HEARTBEAT_FILE).exists());
        drop((first, second));
        fs::remove_dir_all(root).unwrap();
    }
}
