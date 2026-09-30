use std::{
    fs::File,
    io::{self, Read as _, Seek as _, SeekFrom, Write as _},
    thread,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use fs4::{FileExt, TryLockError};
use tracing::{info, warn};

use crate::consts;

/// What a caller about to wait on a lock says about the wait.
#[derive(Clone, Copy, Debug)]
pub struct Wait<'a> {
    /// What the caller is waiting for, as a reader of the log names it.
    pub subject: &'a str,
    /// Who the caller is, recorded in the lock file while it holds it.
    pub holder: &'a str,
}

/// A `flock` held on an open file, released when this drops.
///
/// The release is explicit. The lock lives on the open file description, which
/// every process forked while the descriptor was open holds a duplicate of
/// until it execs, so closing frees nothing while one of those is alive.
/// Unlocking acts on the description and is seen by every duplicate at once.
///
/// A lock taken by waiting never waits in silence: the first failed attempt
/// logs what the caller waits for and who holds it, the wait repeats that
/// every [`consts::LOCK_WAIT_HEARTBEAT`], and the take logs how long it took.
#[derive(Debug)]
pub struct FileLock {
    file: File,
    /// Whether this holder wrote its record into the file.
    recorded: bool,
}

impl FileLock {
    /// Takes `file`'s exclusive lock, waiting out every other holder, and
    /// records `wait.holder` in the file until the lock is released.
    ///
    /// `file` must be a lock file with no content of its own, open for
    /// reading and writing. A record that cannot be written is logged, not
    /// returned: it only names the holder to a waiter, and a job never fails
    /// for its diagnostics.
    ///
    /// # Errors
    ///
    /// When the lock cannot be taken.
    pub fn exclusive(file: File, wait: &Wait<'_>) -> io::Result<Self> {
        acquire(&file, wait.subject, <File as FileExt>::try_lock)?;
        let lock = Self {
            file,
            recorded: true,
        };
        if let Err(error) = lock.record(wait.holder) {
            warn!(
                "took {} without recording its holder, so a waiter cannot name it: {error}",
                wait.subject
            );
        }
        Ok(lock)
    }

    /// Takes `file`'s shared lock, waiting out an exclusive holder.
    ///
    /// `file` must be open for reading and writing: no exclusive holder
    /// exists while this lock is held, so a record in the file names one
    /// that died holding it, and this clears it. A record that cannot be
    /// cleared is logged, not returned: a waiter reading it sees its `since`.
    ///
    /// # Errors
    ///
    /// When the lock cannot be taken.
    pub fn shared(file: File, subject: &str) -> io::Result<Self> {
        acquire(&file, subject, <File as FileExt>::try_lock_shared)?;
        if let Err(error) = clear_record(&file) {
            warn!("took {subject}, but a dead holder's record stays in its lock file: {error}");
        }
        Ok(Self {
            file,
            recorded: false,
        })
    }

    /// Takes `file`'s exclusive lock unless another holder has it.
    ///
    /// # Errors
    ///
    /// [`TryLockError::WouldBlock`] when a holder has the file.
    pub fn try_exclusive(file: File) -> Result<Self, TryLockError> {
        FileExt::try_lock(&file)?;
        Ok(Self {
            file,
            recorded: false,
        })
    }

    /// Takes `file`'s shared lock unless an exclusive holder has it.
    ///
    /// # Errors
    ///
    /// [`TryLockError::WouldBlock`] when a holder has the file.
    pub fn try_shared(file: File) -> Result<Self, TryLockError> {
        FileExt::try_lock_shared(&file)?;
        Ok(Self {
            file,
            recorded: false,
        })
    }

    fn record(&self, holder: &str) -> io::Result<()> {
        let mut file = &self.file;
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        write!(file, "holder={holder}\nsince={}\n", unix_now())
    }
}

/// Empties a lock file of the record a holder left in it.
fn clear_record(file: &File) -> io::Result<()> {
    if file.metadata()?.len() > 0 {
        file.set_len(0)?;
    }
    Ok(())
}

impl Drop for FileLock {
    fn drop(&mut self) {
        if self.recorded {
            let _ = self.file.set_len(0);
        }
        let _ = FileExt::unlock(&self.file);
    }
}

/// Takes the lock `attempt` asks for, trying again every
/// [`consts::LOCK_WAIT_POLL`] and logging the wait.
fn acquire(
    file: &File,
    subject: &str,
    attempt: fn(&File) -> Result<(), TryLockError>,
) -> io::Result<()> {
    let started = Instant::now();
    let mut announced: Option<Instant> = None;
    loop {
        match attempt(file) {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => return Err(error),
        }
        if announced.is_none_or(|at| at.elapsed() >= consts::LOCK_WAIT_HEARTBEAT) {
            warn!(
                "waiting for {subject} ({} s so far), held by {}",
                started.elapsed().as_secs(),
                holder_of(file)
            );
            announced = Some(Instant::now());
        }
        thread::sleep(consts::LOCK_WAIT_POLL);
    }
    if announced.is_some() {
        info!("took {subject} after {} s", started.elapsed().as_secs());
    }
    Ok(())
}

/// Who the record in `file` names, and for how long.
fn holder_of(file: &File) -> String {
    read_record(file).map_or_else(
        |error| format!("a holder whose record is unreadable ({error})"),
        |text| {
            let field = |name: &str| {
                text.lines()
                    .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
            };
            let since = field("since").and_then(|since| since.parse::<u64>().ok());
            field("holder").zip(since).map_or_else(
                || {
                    "a holder that left no record (shared and non-waiting holders write none)"
                        .to_owned()
                },
                |(holder, since)| format!("{holder} for {} s", unix_now().saturating_sub(since)),
            )
        },
    )
}

fn read_record(mut file: &File) -> io::Result<String> {
    let mut text = String::new();
    file.seek(SeekFrom::Start(0))?;
    file.read_to_string(&mut text)?;
    Ok(text)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File, OpenOptions},
        io,
        sync::{Arc, Condvar, Mutex},
        thread,
    };
    #[cfg(unix)]
    use std::{
        process::Command,
        sync::atomic::{AtomicBool, Ordering},
    };

    use fs4::TryLockError;
    use tempfile::TempDir;
    use tracing_subscriber::fmt::MakeWriter;

    use super::{FileLock, Wait};

    fn open(directory: &TempDir) -> File {
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.path().join("lock"))
            .expect("open the lock file")
    }

    fn record(directory: &TempDir) -> String {
        fs::read_to_string(directory.path().join("lock")).expect("read the lock file")
    }

    /// Log text captured from one thread's `tracing` calls.
    #[derive(Clone, Default)]
    struct Sink(Arc<(Mutex<Vec<u8>>, Condvar)>);

    impl Sink {
        fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
            tracing_subscriber::fmt()
                .with_ansi(false)
                .without_time()
                .with_target(false)
                .with_writer(self.clone())
                .finish()
        }

        fn text(&self) -> String {
            let (buffer, _) = &*self.0;
            String::from_utf8_lossy(&buffer.lock().unwrap()).into_owned()
        }

        /// Blocks until a logged line contains `needle` and returns it.
        fn wait_for(&self, needle: &str) -> String {
            let (buffer, logged) = &*self.0;
            let mut bytes = buffer.lock().unwrap();
            loop {
                let found = String::from_utf8_lossy(&bytes)
                    .lines()
                    .find(|line| line.contains(needle))
                    .map(str::to_owned);
                if let Some(line) = found {
                    return line;
                }
                bytes = logged.wait(bytes).unwrap();
            }
        }
    }

    impl io::Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let (buffer, logged) = &*self.0;
            buffer.lock().unwrap().extend_from_slice(bytes);
            logged.notify_all();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Sink {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Takes `file` exclusively on another thread whose log goes to `sink`.
    fn contend(
        sink: &Sink,
        file: File,
        subject: &'static str,
    ) -> thread::JoinHandle<io::Result<()>> {
        let sink = sink.clone();
        thread::spawn(move || {
            tracing::subscriber::with_default(sink.subscriber(), || {
                FileLock::exclusive(
                    file,
                    &Wait {
                        subject,
                        holder: "job-b",
                    },
                )
                .map(drop)
            })
        })
    }

    #[test]
    fn a_holder_refuses_a_second_exclusive_request() {
        let directory = TempDir::new().expect("temporary directory");
        let held = FileLock::try_shared(open(&directory)).expect("take the lock");

        assert!(matches!(
            FileLock::try_exclusive(open(&directory)),
            Err(TryLockError::WouldBlock)
        ));
        drop(held);
    }

    #[test]
    fn an_uncontended_exclusive_lock_is_silent_and_records_its_holder_while_held() {
        let directory = TempDir::new().expect("temporary directory");
        let sink = Sink::default();

        let held = tracing::subscriber::with_default(sink.subscriber(), || {
            FileLock::exclusive(
                open(&directory),
                &Wait {
                    subject: "the test lock",
                    holder: "job-a",
                },
            )
        })
        .expect("take the lock");

        let written = record(&directory);
        assert!(written.starts_with("holder=job-a\nsince="), "{written}");
        assert_eq!(sink.text(), "");
        drop(held);
        assert_eq!(record(&directory), "");
    }

    /// A waiter reads the holder's record from the file, which another
    /// process's exclusive lock keeps unreadable on Windows.
    #[cfg(unix)]
    #[test]
    fn a_contended_lock_names_its_holder_and_reports_the_take() {
        let directory = TempDir::new().expect("temporary directory");
        let first = FileLock::exclusive(
            open(&directory),
            &Wait {
                subject: "the first lock",
                holder: "job-a",
            },
        )
        .expect("take the lock");
        let sink = Sink::default();
        let waiter = contend(&sink, open(&directory), "the test lock");

        let waiting = sink.wait_for("waiting for the test lock");
        assert!(waiting.contains("held by job-a for "), "{waiting}");
        drop(first);
        waiter.join().unwrap().expect("take the released lock");

        let text = sink.text();
        assert!(text.contains("took the test lock after "), "{text}");
    }

    /// A record left by a holder that died with the lock names nobody who
    /// holds it now: shared readers clear it when they take the lock.
    #[test]
    fn a_wait_on_shared_holders_says_they_left_no_record() {
        let directory = TempDir::new().expect("temporary directory");
        fs::write(
            directory.path().join("lock"),
            "holder=a-job-that-died\nsince=1\n",
        )
        .expect("leave a stale record");
        let reader = FileLock::shared(open(&directory), "the journal").expect("take the lock");
        let sink = Sink::default();
        let waiter = contend(&sink, open(&directory), "the test lock");

        let waiting = sink.wait_for("waiting for the test lock");
        assert!(
            waiting.contains("held by a holder that left no record"),
            "{waiting}"
        );
        drop(reader);
        waiter.join().unwrap().expect("take the released lock");
    }

    #[cfg(unix)]
    #[test]
    fn a_released_lock_is_free_while_children_are_being_spawned() {
        let directory = TempDir::new().expect("temporary directory");
        let stop = Arc::new(AtomicBool::new(false));
        let spawner = {
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let mut child = Command::new("/bin/sh")
                        .args(["-c", "exit 0"])
                        .spawn()
                        .expect("spawn a child");
                    child.wait().expect("reap the child");
                }
            })
        };

        let mut still_held = 0;
        for _ in 0..2_000 {
            match FileLock::try_shared(open(&directory)) {
                Ok(held) => drop(held),
                Err(_) => still_held += 1,
            }
            match FileLock::try_exclusive(open(&directory)) {
                Ok(held) => drop(held),
                Err(_) => still_held += 1,
            }
        }
        stop.store(true, Ordering::Relaxed);
        spawner.join().expect("join the spawner");

        assert_eq!(
            still_held, 0,
            "a lock nobody holds must not read as held to the next asker"
        );
    }

    /// The record only names the holder to a waiter: a lock file this holder
    /// cannot write still locks, and the job that took it works on.
    #[test]
    fn a_lock_it_cannot_record_in_still_holds() {
        let directory = TempDir::new().expect("temp dir");
        drop(open(&directory));
        let read_only = File::open(directory.path().join("lock")).expect("open read-only");

        let _held = FileLock::exclusive(
            read_only,
            &Wait {
                subject: "the journal",
                holder: "job-a",
            },
        )
        .expect("the lock is taken without its record");

        assert!(matches!(
            FileLock::try_exclusive(open(&directory)),
            Err(TryLockError::WouldBlock)
        ));
    }

    /// Clearing a dead holder's record is housekeeping: a lock file this
    /// reader cannot write still gives it the shared lock.
    #[test]
    fn a_stale_record_it_cannot_clear_still_gives_the_shared_lock() {
        let directory = TempDir::new().expect("temp dir");
        fs::write(directory.path().join("lock"), "holder=job-dead\nsince=1\n")
            .expect("leave a record");
        let read_only = File::open(directory.path().join("lock")).expect("open read-only");

        let _reader = FileLock::shared(read_only, "the journal")
            .expect("the shared lock is taken without clearing the record");

        assert!(matches!(
            FileLock::try_exclusive(open(&directory)),
            Err(TryLockError::WouldBlock)
        ));
    }
}
