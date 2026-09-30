use std::{
    env,
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::{self, Command, ExitStatus},
    thread,
    time::{Duration, Instant},
};

mod consts {
    use super::Duration;

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
}

impl Heartbeat {
    fn start(lease: &Path) -> io::Result<Self> {
        let parent = lease
            .parent()
            .ok_or_else(|| io::Error::other("lease path has no build directory"))?;
        let heartbeat = Self {
            path: parent.join(consts::HEARTBEAT_FILE),
        };
        heartbeat.refresh()?;
        Ok(heartbeat)
    }

    fn refresh(&self) -> io::Result<()> {
        fs::write(&self.path, format!("pid={}\n", process::id()))
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
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
            Err(fs::TryLockError::Error(error)) => return Err(error),
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
    let lease = PathBuf::from(
        args.next()
            .ok_or_else(|| io::Error::other("missing lease path"))?,
    );
    let command = args
        .next()
        .ok_or_else(|| io::Error::other("missing command"))?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lease)?;
    hold_shared(&file, &lease)?;
    let heartbeat = Heartbeat::start(&lease)?;
    let _ = env::current_exe().and_then(fs::remove_file);
    let mut child = Command::new(command).args(args).spawn()?;
    let mut refresh_at = Instant::now() + consts::HEARTBEAT_INTERVAL;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
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
