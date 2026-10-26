#[cfg(unix)]
use std::time::Duration;
use std::{
    fs::File,
    io::Write as _,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::Instant,
};

use anyhow::{Context, Result, bail};
use tracing::{info, warn};

use crate::{
    child::{self, Cancel},
    ci::{config::CiConfig, process::Process},
    consts,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Outcome {
    Completed,
    Failed,
    TimedOut,
    Contended,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Operation {
    Cleanup,
    Check,
}

impl Outcome {
    const fn byte(self) -> u8 {
        match self {
            Self::Completed => 0,
            Self::Failed => 1,
            Self::TimedOut => 2,
            Self::Contended => 3,
            Self::Cancelled => 4,
        }
    }
}

pub(super) fn preflight(
    config: &CiConfig,
    process: &Process,
    profile: &Path,
    pins: &Path,
) -> Result<()> {
    let cancel = Cancel::install()?;
    let total = config.host.job_admission_deadline();
    let deadline = Instant::now()
        .checked_add(total)
        .context("admission deadline overflow")?;
    let maintenance_deadline = deadline - (2 * consts::CONTROL_TIMEOUT).min(total / 2);
    let outcome = request(
        process,
        profile,
        pins,
        Operation::Cleanup,
        maintenance_deadline,
        &cancel,
    )?;
    match outcome {
        Outcome::Completed => info!(?outcome, "admission maintenance finished"),
        Outcome::Cancelled => bail!("admission maintenance was cancelled"),
        _ => warn!(?outcome, "optional admission maintenance did not complete"),
    }
    child::check(Some(&cancel))?;
    let outcome = request(process, profile, pins, Operation::Check, deadline, &cancel)?;
    if outcome != Outcome::Completed {
        bail!("required host checks did not complete: {outcome:?}");
    }
    child::check(Some(&cancel))
}

pub(super) fn cleanup(
    config: &CiConfig,
    process: &Process,
    profile: &Path,
    pins: &Path,
) -> Result<()> {
    let cancel = Cancel::install()?;
    let deadline = Instant::now()
        .checked_add(config.host.cleanup_deadline())
        .context("maintenance deadline overflow")?;
    let outcome = request(
        process,
        profile,
        pins,
        Operation::Cleanup,
        deadline,
        &cancel,
    )?;
    info!(?outcome, "host maintenance finished");
    if outcome != Outcome::Completed {
        bail!("host maintenance did not complete: {outcome:?}");
    }
    child::check(Some(&cancel))
}

fn invocation(process: &Process, profile: &Path, pins: &Path, operation: &str) -> Result<Command> {
    let mut command = process
        .command(std::env::current_exe().context("locating installed maintenance executable")?);
    command
        .args(["ci", "host", "mac", "--config"])
        .arg(profile)
        .arg("--pins")
        .arg(pins)
        .arg(operation);
    Ok(command)
}

fn request(
    process: &Process,
    profile: &Path,
    pins: &Path,
    operation: Operation,
    deadline: Instant,
    cancel: &Cancel,
) -> Result<Outcome> {
    child::check(Some(cancel))?;
    let total = deadline.saturating_duration_since(Instant::now());
    let work_deadline = deadline - consts::CONTROL_TIMEOUT.min(total / 2);
    let mut command = invocation(process, profile, pins, "maintenance")?;
    command.arg(
        work_deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .to_string(),
    );
    if operation == Operation::Check {
        command.arg("--check");
    }
    command.stdin(Stdio::piped()).stdout(Stdio::piped());
    launch_request(
        move || child::spawn(&mut command),
        cancel,
        work_deadline,
        deadline,
    )
}

fn launch_request(
    launch: impl FnOnce() -> Result<Child> + Send + 'static,
    cancel: &Cancel,
    work_deadline: Instant,
    deadline: Instant,
) -> Result<Outcome> {
    let (finished, outcome) = mpsc::channel();
    let (stop, requested) = mpsc::channel::<()>();
    thread::spawn(move || {
        let mut supervisor = match launch() {
            Ok(supervisor) => supervisor,
            Err(error) => {
                let _ = finished.send(Err(error));
                return;
            }
        };
        let result = (|| {
            let control = supervisor
                .stdin
                .take()
                .context("maintenance control pipe")?;
            let result = supervisor
                .stdout
                .take()
                .context("maintenance result pipe")?;
            wait_supervisor(
                &mut supervisor,
                control,
                result,
                || !matches!(requested.try_recv(), Err(mpsc::TryRecvError::Empty)),
                work_deadline,
                deadline,
            )
        })();
        drop(supervisor.stdin.take());
        let _ = finished.send(result);
        // A late launch still owns its child; EOF requests stop without killing the supervisor.
        loop {
            if matches!(supervisor.try_wait(), Ok(Some(_))) {
                break;
            }
            thread::sleep(consts::CHILD_POLL);
        }
    });
    let mut stop = Some(stop);
    loop {
        if child::check(Some(cancel)).is_err() || Instant::now() >= work_deadline {
            drop(stop.take());
        }
        if Instant::now() >= deadline {
            bail!(
                "maintenance launch or stop is unconfirmed; admission refused and lifecycle owner retained"
            );
        }
        match outcome.try_recv() {
            Ok(result) => {
                child::check(Some(cancel))?;
                return result;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                bail!("maintenance lifecycle owner ended without confirmation")
            }
        }
        thread::sleep(consts::CHILD_POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

fn wait_supervisor(
    supervisor: &mut Child,
    control: ChildStdin,
    mut result: impl std::io::Read + Send + 'static,
    mut stop: impl FnMut() -> bool,
    work_deadline: Instant,
    deadline: Instant,
) -> Result<Outcome> {
    let (send, confirmation) = mpsc::channel();
    thread::spawn(move || {
        let mut byte = [0];
        let result = result.read_exact(&mut byte).map(|()| byte[0]);
        let _ = send.send(result);
    });
    let mut control = Some(control);
    let mut status = None;
    loop {
        if stop() || Instant::now() >= work_deadline {
            drop(control.take());
        }
        if Instant::now() >= deadline {
            bail!(
                "maintenance stop is unconfirmed; admission refused and supervisor retains exclusion"
            );
        }
        if status.is_none() {
            status = supervisor
                .try_wait()
                .context("waiting for maintenance supervisor")?;
        }
        if let Some(status) = status {
            if !status.success() {
                bail!("maintenance supervisor exited without safe-stop confirmation");
            }
            let byte = match confirmation.try_recv() {
                Ok(result) => {
                    Some(result.context("maintenance exited without safe-stop confirmation")?)
                }
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    bail!("maintenance exited without safe-stop confirmation")
                }
            };
            if let Some(byte) = byte {
                return match byte {
                    0 => Ok(Outcome::Completed),
                    1 => Ok(Outcome::Failed),
                    2 => Ok(Outcome::TimedOut),
                    3 => Ok(Outcome::Contended),
                    4 => Ok(Outcome::Cancelled),
                    _ => bail!("invalid maintenance safe-stop confirmation"),
                };
            }
        }
        thread::sleep(consts::CHILD_POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

pub(super) fn supervisor(
    config: &CiConfig,
    process: &Process,
    profile: &Path,
    pins: &Path,
    operation: Operation,
    work_budget_ms: u64,
) -> Result<()> {
    #[cfg(unix)]
    let outcome = unix::maintain(
        config,
        process,
        profile,
        pins,
        operation,
        Duration::from_millis(work_budget_ms),
        std::io::stdin(),
    );
    #[cfg(not(unix))]
    let outcome: Result<Outcome> = {
        let _ = (config, process, profile, pins, operation, work_budget_ms);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Mac maintenance requires Unix process groups",
        )
        .into())
    };
    let outcome = outcome.unwrap_or_else(|_| {
        warn!(reason = "setup", "host maintenance could not start");
        Outcome::Failed
    });
    std::io::stdout()
        .write_all(&[outcome.byte()])
        .context("confirming safe maintenance stop")
}

pub(super) fn validate_pass(config: &CiConfig) -> Result<File> {
    #[cfg(unix)]
    {
        unix::validate_pass(config)
    }
    #[cfg(not(unix))]
    {
        let _ = config;
        bail!("Mac maintenance requires Unix process groups");
    }
}

#[cfg(unix)]
mod unix {
    use std::{
        fs::{self, File, OpenOptions},
        io::Read,
        os::{
            fd::AsFd as _,
            unix::fs::{MetadataExt as _, OpenOptionsExt as _},
        },
        sync::mpsc::{self, Receiver, TryRecvError},
    };

    use fs4::{FileExt, TryLockError};
    use kithara_devtools::lock::FileLock;
    use nix::libc::O_NOFOLLOW;

    use super::*;

    pub(super) fn validate_pass(config: &CiConfig) -> Result<File> {
        let proof = File::from(std::io::stdin().as_fd().try_clone_to_owned()?);
        validate_proof(&config.host.host_root, proof)
    }

    fn validate_proof(root: &Path, proof: File) -> Result<File> {
        let held = proof
            .metadata()
            .context("reading inherited maintenance proof")?;
        let installed = lock_file(root)?.metadata()?;
        if !held.is_file() || held.dev() != installed.dev() || held.ino() != installed.ino() {
            bail!("cleanup pass requires its supervisor's installed lock descriptor");
        }
        FileExt::try_lock(&proof)
            .context("cleanup pass requires exclusive maintenance ownership")?;
        // Closing this duplicate must not explicitly unlock the supervisor's description.
        Ok(proof)
    }

    fn lock_file(root: &Path) -> Result<File> {
        for directory in [root.to_path_buf(), root.join("locks")] {
            if !fs::symlink_metadata(directory)?.is_dir() {
                bail!("maintenance lock root must be an installed nonsymlink directory");
            }
        }
        let path = root.join("locks/cleanup.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(O_NOFOLLOW)
            .open(&path)
            .context("opening installed maintenance lock")?;
        let metadata = file.metadata()?;
        let installed = fs::symlink_metadata(path)?;
        if !metadata.is_file()
            || !installed.is_file()
            || metadata.nlink() != 1
            || metadata.dev() != installed.dev()
            || metadata.ino() != installed.ino()
        {
            bail!("maintenance lock must be a stable installed regular file");
        }
        Ok(file)
    }

    fn disconnected(mut input: impl Read + Send + 'static) -> Receiver<()> {
        let (send, receive) = mpsc::channel();
        thread::spawn(move || {
            let _ = input.read(&mut [0]);
            let _ = send.send(());
        });
        receive
    }

    fn disconnected_now(control: &Receiver<()>) -> bool {
        !matches!(control.try_recv(), Err(TryRecvError::Empty))
    }

    pub(super) fn maintain(
        config: &CiConfig,
        process: &Process,
        profile: &Path,
        pins: &Path,
        operation: Operation,
        budget: Duration,
        input: impl Read + Send + 'static,
    ) -> Result<Outcome> {
        let cancel = Cancel::install()?;
        let deadline = Instant::now()
            .checked_add(budget)
            .context("maintenance deadline overflow")?;
        let control = disconnected(input);
        if operation == Operation::Check {
            let mut check = invocation(process, profile, pins, "preflight-check")?;
            check.stdin(Stdio::null()).stdout(Stdio::null());
            return oversee(&mut check, &control, &cancel, deadline);
        }
        let mut contended = false;
        let (lock, proof) = loop {
            if child::check(Some(&cancel)).is_err() {
                return Ok(Outcome::Cancelled);
            }
            if disconnected_now(&control) || Instant::now() >= deadline {
                return Ok(if contended {
                    Outcome::Contended
                } else {
                    Outcome::TimedOut
                });
            }
            let file = lock_file(&config.host.host_root)?;
            let proof = file.try_clone()?;
            match FileLock::try_exclusive(file) {
                Ok(lock) => break (lock, proof),
                Err(TryLockError::WouldBlock) => {
                    contended = true;
                    thread::sleep(
                        consts::CHILD_POLL.min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
                Err(TryLockError::Error(error)) => {
                    return Err(error).context("taking maintenance lock");
                }
            }
        };
        if child::check(Some(&cancel)).is_err() {
            return Ok(Outcome::Cancelled);
        }
        if disconnected_now(&control) || Instant::now() >= deadline {
            return Ok(Outcome::TimedOut);
        }
        let mut pass = invocation(process, profile, pins, "cleanup-pass")?;
        pass.arg(
            deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .to_string(),
        );
        pass.stdin(proof).stdout(Stdio::null());
        let outcome = oversee(&mut pass, &control, &cancel, deadline)?;
        drop(lock);
        Ok(outcome)
    }

    fn oversee(
        command: &mut Command,
        control: &Receiver<()>,
        cancel: &Cancel,
        deadline: Instant,
    ) -> Result<Outcome> {
        if child::check(Some(cancel)).is_err() {
            return Ok(Outcome::Cancelled);
        }
        if disconnected_now(control) || Instant::now() >= deadline {
            return Ok(Outcome::TimedOut);
        }
        let mut pass = child::spawn(command)?;
        let outcome = loop {
            if child::check(Some(cancel)).is_err() {
                break Some(Outcome::Cancelled);
            }
            if disconnected_now(control) || Instant::now() >= deadline {
                break Some(Outcome::TimedOut);
            }
            match child::leader_exited(&pass) {
                Ok(true) => break None,
                Ok(false) => thread::sleep(
                    consts::CHILD_POLL.min(deadline.saturating_duration_since(Instant::now())),
                ),
                Err(_) => break Some(Outcome::Failed),
            }
        };
        let status = child::stop_group(&mut pass, Instant::now() + consts::CONTROL_TIMEOUT);
        if let Ok(status) = status {
            return Ok(outcome.unwrap_or_else(|| {
                if status.success() {
                    Outcome::Completed
                } else {
                    Outcome::Failed
                }
            }));
        }
        warn!(
            reason = "unconfirmed-stop",
            "maintenance exclusion retained until group stop is confirmed"
        );
        loop {
            if let Ok(Some(status)) = child::group_stopped(&mut pass) {
                return Ok(outcome.unwrap_or_else(|| {
                    if status.success() {
                        Outcome::Completed
                    } else {
                        Outcome::Failed
                    }
                }));
            }
            thread::sleep(consts::CHILD_POLL);
        }
    }

    #[cfg(test)]
    mod tests {
        use std::{
            io::Cursor,
            os::unix::{net::UnixStream, process::CommandExt as _},
        };

        use super::*;
        use crate::{ci::config::fixture, testing::signals};

        fn root() -> tempfile::TempDir {
            let root = tempfile::tempdir().unwrap();
            fs::create_dir(root.path().join("locks")).unwrap();
            fs::write(root.path().join("locks/cleanup.lock"), "").unwrap();
            root
        }

        fn until(mut predicate: impl FnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !predicate() {
                assert!(Instant::now() < deadline, "fixture did not reach its state");
                thread::sleep(consts::CHILD_POLL);
            }
        }

        fn fixture_command(root: &Path, check: bool) -> Child {
            child::spawn(
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "ci::host::mac::admission::unix::tests::supervisor_fixture",
                        "--nocapture",
                    ])
                    .env("KITHARA_MAINTENANCE_FIXTURE", root)
                    .env("KITHARA_MAINTENANCE_CHECK_FIXTURE", check.to_string())
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null()),
            )
            .unwrap()
        }

        #[test]
        fn supervisor_fixture() {
            let Some(root) = std::env::var_os("KITHARA_MAINTENANCE_FIXTURE") else {
                return;
            };
            let root = Path::new(&root);
            let cancel = Cancel::install().unwrap();
            let control = disconnected(std::io::stdin());
            let lock =
                if std::env::var("KITHARA_MAINTENANCE_CHECK_FIXTURE").as_deref() == Ok("true") {
                    None
                } else {
                    Some(FileLock::try_exclusive(lock_file(root).unwrap()).unwrap())
                };
            let outcome = oversee(Command::new("sh")
                .args(["-c", "echo $$ > \"$1/pid\"; while [ ! -f \"$1/release\" ]; do sleep 0.02; done; exit 0", "sh"])
                .arg(root).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()),
                &control, &cancel, Instant::now() + Duration::from_secs(30)).unwrap();
            drop(lock);
            fs::write(root.join("outcome"), outcome.byte().to_string()).unwrap();
        }

        #[test]
        fn an_unreaped_descendant_outlives_the_leader_and_frontend_budget() {
            let _signals = signals();
            let root = root();
            let mut supervisor = fixture_command(root.path(), false);
            until(|| {
                fs::read_to_string(root.path().join("pid"))
                    .is_ok_and(|text| text.trim().parse::<i32>().is_ok())
            });
            let group: i32 = fs::read_to_string(root.path().join("pid"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            let mut descendant = Command::new("sleep")
                .arg("30")
                .process_group(group)
                .spawn()
                .unwrap();
            fs::write(root.path().join("release"), "").unwrap();
            until(|| child::leader_exited(&descendant).unwrap());

            let cancel = Cancel::install().unwrap();
            let now = Instant::now();
            let control = supervisor.stdin.take().unwrap();
            let error = wait_supervisor(
                &mut supervisor,
                control,
                Cursor::new([0]),
                || child::check(Some(&cancel)).is_err(),
                now + Duration::from_millis(40),
                now + Duration::from_millis(150),
            )
            .unwrap_err();
            assert!(error.to_string().contains("unconfirmed"));
            assert!(now.elapsed() < Duration::from_secs(1));
            assert!(supervisor.try_wait().unwrap().is_none());
            assert!(matches!(
                FileLock::try_exclusive(lock_file(root.path()).unwrap()),
                Err(TryLockError::WouldBlock)
            ));

            descendant.wait().unwrap();
            until(|| supervisor.try_wait().unwrap().is_some());
            assert_eq!(
                fs::read_to_string(root.path().join("outcome")).unwrap(),
                "0"
            );
            assert!(FileLock::try_exclusive(lock_file(root.path()).unwrap()).is_ok());
        }

        #[test]
        fn frontend_death_stops_its_pass_and_contention_preserves_the_peer() {
            let _signals = signals();
            let root = root();
            let mut peer = fixture_command(root.path(), false);
            until(|| {
                fs::read_to_string(root.path().join("pid"))
                    .is_ok_and(|text| text.trim().parse::<i32>().is_ok())
            });
            let mut config = fixture();
            config.host.host_root = root.path().to_path_buf();
            let process = Process::new(root.path(), std::collections::BTreeMap::new());
            let (reader, writer) = UnixStream::pair().unwrap();
            let now = Instant::now();
            assert_eq!(
                maintain(
                    &config,
                    &process,
                    Path::new("profile"),
                    Path::new("pins"),
                    Operation::Cleanup,
                    Duration::from_millis(100),
                    reader
                )
                .unwrap(),
                Outcome::Contended
            );
            assert!(now.elapsed() < Duration::from_secs(1));
            assert!(peer.try_wait().unwrap().is_none());
            drop(writer);
            drop(peer.stdin.take());
            until(|| peer.try_wait().unwrap().is_some());
            assert_eq!(
                fs::read_to_string(root.path().join("outcome")).unwrap(),
                "2"
            );
            assert!(FileLock::try_exclusive(lock_file(root.path()).unwrap()).is_ok());
        }

        #[test]
        fn unsafe_or_untrusted_completion_never_becomes_an_admission_warning() {
            let _signals = signals();
            let cancel = Cancel::install().unwrap();
            let mut process = child::spawn(
                Command::new("sh")
                    .args(["-c", "exit 0"])
                    .stdin(Stdio::piped()),
            )
            .unwrap();
            let control = process.stdin.take().unwrap();
            let now = Instant::now();
            let error = wait_supervisor(
                &mut process,
                control,
                Cursor::new(b"secret-token"),
                || child::check(Some(&cancel)).is_err(),
                now + Duration::from_secs(1),
                now + Duration::from_secs(2),
            )
            .unwrap_err();
            assert!(error.to_string().contains("invalid"));
            assert!(!error.to_string().contains("secret-token"));
        }

        #[test]
        fn a_result_pipe_held_by_an_unrelated_process_cannot_extend_the_deadline() {
            let _signals = signals();
            let cancel = Cancel::install().unwrap();
            let (reader, writer) = UnixStream::pair().unwrap();
            let writer: std::os::fd::OwnedFd = writer.into();
            let mut unrelated = Command::new("sleep")
                .arg("30")
                .stdout(Stdio::from(writer))
                .spawn()
                .unwrap();
            let mut supervisor = child::spawn(
                Command::new("sh")
                    .args(["-c", "exit 0"])
                    .stdin(Stdio::piped()),
            )
            .unwrap();
            let control = supervisor.stdin.take().unwrap();
            let now = Instant::now();
            let error = wait_supervisor(
                &mut supervisor,
                control,
                reader,
                || child::check(Some(&cancel)).is_err(),
                now + Duration::from_millis(40),
                now + Duration::from_millis(150),
            )
            .unwrap_err();
            assert!(error.to_string().contains("unconfirmed"));
            assert!(now.elapsed() < Duration::from_secs(1));
            assert!(unrelated.try_wait().unwrap().is_none());
            unrelated.kill().unwrap();
            unrelated.wait().unwrap();
        }

        #[test]
        fn substituted_descriptor_is_rejected_while_a_peer_owns_exclusion() {
            let root = root();
            let peer = FileLock::try_exclusive(lock_file(root.path()).unwrap()).unwrap();
            assert!(validate_proof(root.path(), lock_file(root.path()).unwrap()).is_err());
            assert!(matches!(
                FileLock::try_exclusive(lock_file(root.path()).unwrap()),
                Err(TryLockError::WouldBlock)
            ));
            drop(peer);
            assert!(FileLock::try_exclusive(lock_file(root.path()).unwrap()).is_ok());
        }

        #[test]
        fn inherited_proof_never_unlocks_its_supervisor_and_direct_proof_stays_held() {
            let root = root();
            let file = lock_file(root.path()).unwrap();
            let proof = file.try_clone().unwrap();
            let supervisor = FileLock::try_exclusive(file).unwrap();
            drop(validate_proof(root.path(), proof).unwrap());
            assert!(matches!(
                FileLock::try_exclusive(lock_file(root.path()).unwrap()),
                Err(TryLockError::WouldBlock)
            ));
            drop(supervisor);

            let proof = validate_proof(root.path(), lock_file(root.path()).unwrap()).unwrap();
            assert!(matches!(
                FileLock::try_exclusive(lock_file(root.path()).unwrap()),
                Err(TryLockError::WouldBlock)
            ));
            drop(proof);
            assert!(FileLock::try_exclusive(lock_file(root.path()).unwrap()).is_ok());
        }

        #[test]
        fn delayed_cleanup_and_check_launches_refuse_then_reap_the_late_supervisor() {
            let _signals = signals();
            let cancel = Cancel::install().unwrap();
            for check in [false, true] {
                let root = root();
                let path = root.path().to_path_buf();
                let (known, launched) = mpsc::channel();
                let now = Instant::now();
                let error = launch_request(
                    move || {
                        thread::sleep(Duration::from_secs(2));
                        let supervisor = fixture_command(&path, check);
                        known.send(supervisor.id()).unwrap();
                        Ok(supervisor)
                    },
                    &cancel,
                    now + Duration::from_millis(50),
                    now + Duration::from_millis(150),
                )
                .unwrap_err();
                assert!(error.to_string().contains("unconfirmed"));
                assert!(now.elapsed() < Duration::from_secs(1));
                let pid = nix::unistd::Pid::from_raw(
                    i32::try_from(launched.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap(),
                );
                until(|| nix::sys::signal::kill(pid, None) == Err(nix::errno::Errno::ESRCH));
                assert_eq!(
                    fs::read_to_string(root.path().join("outcome")).unwrap(),
                    "2"
                );
                assert!(FileLock::try_exclusive(lock_file(root.path()).unwrap()).is_ok());
            }
        }
    }
}
