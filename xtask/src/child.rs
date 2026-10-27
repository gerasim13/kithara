//! Owned process groups, cancellation, and deadlines for control commands.

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;
use std::{
    process::{Child, Command, ExitStatus, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
#[cfg(unix)]
use nix::{
    errno::Errno,
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use signal_hook::{
    SigId,
    consts::signal::{SIGINT, SIGTERM},
    flag, low_level,
};

use crate::consts;

/// Children of a run get their own process group, including commands they spawn.
#[cfg(unix)]
pub(crate) fn isolate(command: &mut Command) {
    command.process_group(0);
}

#[cfg(not(unix))]
pub(crate) fn isolate(_command: &mut Command) {}

pub(crate) fn spawn(command: &mut Command) -> Result<Child> {
    isolate(command);
    command.spawn().context("spawning owned command")
}

/// Stop the owned group even when its leader has already exited.
#[cfg(unix)]
fn kill_owned(child: &mut Child) -> Result<()> {
    let pid = i32::try_from(child.id()).context("child process group id")?;
    match killpg(Pid::from_raw(pid), Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(error).context("stopping owned process group"),
    }
}

/// Observe exit without releasing the PID that anchors the owned group.
#[cfg(unix)]
pub(crate) fn leader_exited(child: &Child) -> Result<bool> {
    let mut status = std::mem::MaybeUninit::<nix::libc::siginfo_t>::zeroed();
    // SAFETY: waitid initializes status; WNOWAIT preserves this unreaped child's PID.
    let result = unsafe {
        nix::libc::waitid(
            nix::libc::P_PID,
            child.id(),
            status.as_mut_ptr(),
            nix::libc::WEXITED | nix::libc::WNOHANG | nix::libc::WNOWAIT,
        )
    };
    if result < 0 {
        return Err(std::io::Error::last_os_error()).context("observing owned group leader");
    }
    // SAFETY: status was initialized above, including zero si_pid when no child exited.
    Ok(unsafe { status.assume_init().si_pid() } != 0)
}

/// Only observe after the final signal: a reaped leader's PID may be reused.
#[cfg(unix)]
pub(crate) fn group_stopped(child: &mut Child) -> Result<Option<ExitStatus>> {
    let Some(status) = child.try_wait().context("reaping owned group leader")? else {
        return Ok(None);
    };
    let pid = i32::try_from(child.id()).context("child process group id")?;
    match killpg(Pid::from_raw(pid), None) {
        Err(Errno::ESRCH) => Ok(Some(status)),
        Ok(()) => Ok(None),
        Err(error) => Err(error).context("checking owned process group"),
    }
}

/// Send the last group signal before reaping its leader, then only observe.
#[cfg(unix)]
pub(crate) fn stop_group(child: &mut Child, deadline: Instant) -> Result<ExitStatus> {
    if leader_exited(child).is_ok() {
        let _ = kill_owned(child);
    }
    loop {
        if Instant::now() >= deadline {
            bail!("owned process group stop is unconfirmed");
        }
        if let Ok(Some(status)) = group_stopped(child) {
            return Ok(status);
        }
        thread::sleep(consts::CHILD_POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

#[cfg(not(unix))]
fn kill_owned(child: &mut Child) -> Result<()> {
    let mut taskkill = Command::new("taskkill")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .spawn()
        .context("stopping owned process tree")?;
    let status = if let Some(status) = reap(&mut taskkill, consts::GRACE, "taskkill")? {
        status
    } else {
        taskkill.kill().context("stopping timed-out taskkill")?;
        reap(&mut taskkill, consts::GRACE, "taskkill")?.context("taskkill did not exit")?;
        bail!("taskkill exceeded its deadline");
    };
    if !status.success() && child.try_wait()?.is_none() {
        bail!("taskkill failed: {status}");
    }
    Ok(())
}

pub(crate) fn stop(child: &mut Child, what: &str) -> Result<ExitStatus> {
    kill_owned(child).with_context(|| format!("stopping {what}"))?;
    reap(child, consts::GRACE, what)?.with_context(|| {
        format!(
            "{what} outlived the {}s stop grace",
            consts::GRACE.as_secs()
        )
    })
}

/// Asks the owned group to stop, and kills it if its leader is still running
/// once `grace` is up.
#[cfg(unix)]
pub(crate) fn terminate(child: &mut Child, grace: Duration) -> Result<ExitStatus> {
    let pid = i32::try_from(child.id()).context("child process group id")?;
    match killpg(Pid::from_raw(pid), Signal::SIGTERM) {
        Ok(()) | Err(Errno::ESRCH) => {}
        Err(error) => return Err(error).context("asking the owned group to stop"),
    }
    reap(child, grace, "terminated command")?.map_or_else(|| stop(child, "terminated command"), Ok)
}

/// Windows has no request to stop a process tree, only the kill.
#[cfg(not(unix))]
pub(crate) fn terminate(child: &mut Child, _grace: Duration) -> Result<ExitStatus> {
    stop(child, "terminated command")
}

pub(crate) fn check(cancel: Option<&Cancel>) -> Result<()> {
    let signal = cancel.map_or(0, |cancel| cancel.pending.load(Ordering::SeqCst));
    if signal != 0 {
        bail!("run cancelled by signal {signal}");
    }
    Ok(())
}

/// A deadline is required for control commands, while builds end on cancellation.
pub(crate) fn supervise(
    child: &mut Child,
    cancel: Option<&Cancel>,
    timeout: Option<Duration>,
) -> Result<ExitStatus> {
    let started = Instant::now();
    loop {
        let proceeding = check(cancel).and_then(|()| {
            if timeout.is_some_and(|limit| started.elapsed() >= limit) {
                bail!("owned command exceeded its deadline");
            }
            Ok(())
        });
        if let Err(error) = proceeding {
            let stopped = stop(child, "cancelled or timed-out command");
            return Err(match stopped {
                Ok(_) => error,
                Err(stop_error) => error.context(stop_error),
            });
        }
        match child.try_wait().context("waiting on owned command") {
            Ok(Some(status)) => {
                kill_owned(child)?;
                return Ok(status);
            }
            Ok(None) => thread::sleep(consts::CHILD_POLL),
            Err(error) => {
                return Err(match stop(child, "failed command wait") {
                    Ok(_) => error,
                    Err(cleanup) => error.context(cleanup),
                });
            }
        }
    }
}

pub(crate) fn run(command: &mut Command, cancel: Option<&Cancel>) -> Result<ExitStatus> {
    check(cancel)?;
    supervise(&mut spawn(command)?, cancel, None)
}

/// Drain both pipes while the command runs, so a full pipe cannot prevent exit.
pub(crate) fn output(
    command: &mut Command,
    cancel: Option<&Cancel>,
    timeout: Duration,
) -> Result<Output> {
    check(cancel)?;
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = spawn(command)?;
    let stdout = child.stdout.take().context("captured stdout")?;
    let stderr = child.stderr.take().context("captured stderr")?;
    let stdout = read_pipe(stdout);
    let stderr = read_pipe(stderr);
    let status = supervise(&mut child, cancel, Some(timeout))?;
    let stdout = stdout
        .recv_timeout(consts::GRACE)
        .context("draining command stdout")??;
    let stderr = stderr
        .recv_timeout(consts::GRACE)
        .context("draining command stderr")??;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Keep descendants in the enclosing pass group, whose supervisor proves stop.
pub(crate) fn member_output(
    command: &mut Command,
    cancel: Option<&Cancel>,
    timeout: Duration,
) -> Result<Output> {
    check(cancel)?;
    let deadline = Instant::now() + timeout;
    let work_deadline = deadline - consts::CONTROL_TIMEOUT.min(timeout / 2);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().context("spawning pass command")?;
    let stdout = read_pipe(child.stdout.take().context("captured stdout")?);
    let stderr = read_pipe(child.stderr.take().context("captured stderr")?);
    let status = (|| loop {
        check(cancel)?;
        if Instant::now() >= work_deadline {
            bail!("pass command exceeded its deadline");
        }
        if let Some(status) = child.try_wait().context("waiting on pass command")? {
            return Ok(status);
        }
        thread::sleep(
            consts::CHILD_POLL.min(work_deadline.saturating_duration_since(Instant::now())),
        );
    })();
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            child.kill().context("stopping pass command")?;
            reap(
                &mut child,
                deadline.saturating_duration_since(Instant::now()),
                "pass command",
            )?
            .context("pass command stop is unconfirmed")?;
            return Err(error);
        }
    };
    let stdout = stdout
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .context("draining pass stdout")??;
    let stderr = stderr
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .context("draining pass stderr")??;
    if Instant::now() >= deadline {
        bail!("pass command exceeded its deadline");
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn read_pipe(
    mut stream: impl std::io::Read + Send + 'static,
) -> mpsc::Receiver<std::io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stream.read_to_end(&mut bytes).map(|_| bytes);
        let _ = sender.send(result);
    });
    receiver
}

fn reap(child: &mut Child, grace: Duration, what: &str) -> Result<Option<ExitStatus>> {
    let deadline = Instant::now() + grace;
    loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("waiting on {what}"))?
        {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(consts::CHILD_POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

/// Record SIGINT/SIGTERM until the run has released its resources.
/// Dropping registrations does not restore default signal dispositions.
pub(crate) struct Cancel {
    pending: Arc<AtomicUsize>,
    registrations: Vec<SigId>,
}

impl Cancel {
    pub(crate) fn install() -> Result<Self> {
        let pending = Arc::new(AtomicUsize::new(0));
        let mut registrations = Vec::new();
        for signal in [SIGINT, SIGTERM] {
            let value = usize::try_from(signal).context("signal number out of range")?;
            let registration = flag::register_usize(signal, Arc::clone(&pending), value)
                .with_context(|| format!("installing the handler for signal {signal}"))?;
            registrations.push(registration);
        }
        Ok(Self {
            pending,
            registrations,
        })
    }
}

impl Drop for Cancel {
    fn drop(&mut self) {
        for registration in self.registrations.drain(..) {
            low_level::unregister(registration);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::{io::Read as _, os::unix::process::ExitStatusExt as _};

    use super::*;
    use crate::testing::signals;

    fn shell(script: &str) -> Child {
        spawn(
            Command::new("sh")
                .args(["-c", script])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .expect("sh spawns")
    }

    /// A store asked to stop finishes its writes; one killed loses them.
    #[test]
    fn terminate_lets_the_command_stop_on_its_own() {
        let mut child = spawn(
            Command::new("sh")
                .args([
                    "-c",
                    "trap 'exit 0' TERM; echo ready; while :; do sleep 0.05; done",
                ])
                .stdout(Stdio::piped()),
        )
        .unwrap();
        let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        std::io::BufRead::read_line(&mut stdout, &mut ready).unwrap();
        assert_eq!(ready, "ready\n");

        let status = terminate(&mut child, consts::GRACE).unwrap();

        assert_eq!(status.code(), Some(0), "{status}");
    }

    /// A command that does not stop when asked is killed once its grace is
    /// up, so whoever asked is never held by it.
    #[test]
    fn terminate_kills_a_command_that_outlives_its_grace() {
        let mut child = spawn(
            Command::new("sh")
                .args([
                    "-c",
                    "trap '' TERM; echo ready; while :; do sleep 0.05; done",
                ])
                .stdout(Stdio::piped()),
        )
        .unwrap();
        let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        std::io::BufRead::read_line(&mut stdout, &mut ready).unwrap();
        assert_eq!(ready, "ready\n");

        let status = terminate(&mut child, Duration::from_millis(200)).unwrap();

        assert_eq!(status.signal(), Some(Signal::SIGKILL as i32), "{status}");
    }

    #[test]
    fn supervised_command_returns_its_exit_status() {
        let mut child = shell("exit 7");
        assert_eq!(
            supervise(&mut child, None, Some(Duration::from_secs(5)))
                .unwrap()
                .code(),
            Some(7)
        );
    }

    #[test]
    fn an_early_leader_exit_releases_its_descendants() {
        let mut child = spawn(
            Command::new("sh")
                .args(["-c", "sleep 30 & echo ready; exit 7"])
                .stdout(Stdio::piped()),
        )
        .unwrap();
        let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        std::io::BufRead::read_line(&mut stdout, &mut ready).unwrap();
        assert_eq!(ready, "ready\n");
        assert_eq!(supervise(&mut child, None, None).unwrap().code(), Some(7));
        let (closed, receive) = mpsc::channel();
        thread::spawn(move || {
            let mut bytes = Vec::new();
            closed.send(stdout.read_to_end(&mut bytes)).unwrap();
        });
        receive
            .recv_timeout(Duration::from_secs(2))
            .expect("descendant outlived the command")
            .unwrap();
    }

    #[test]
    fn control_command_deadline_stops_a_pipe_holding_descendant() {
        let start = Instant::now();
        let result = output(
            Command::new("sh").args(["-c", "sleep 30 & wait"]),
            None,
            Duration::from_millis(100),
        );
        assert!(result.unwrap_err().to_string().contains("deadline"));
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_pass_control_command_inherits_its_process_group() {
        let output = member_output(
            Command::new("sh").args(["-c", "ps -o pgid= -p $$"]),
            None,
            Duration::from_secs(2),
        )
        .unwrap();
        let group: i32 = String::from_utf8(output.stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(group, nix::unistd::getpgrp().as_raw());
    }

    #[test]
    fn pass_command_stop_and_pipe_drain_share_the_total_budget() {
        // waitpid(-1) must not observe another libtest thread's child.
        if std::env::var_os("KITHARA_PASS_BUDGET_FIXTURE").is_none() {
            assert!(
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "child::tests::pass_command_stop_and_pipe_drain_share_the_total_budget",
                        "--nocapture",
                    ])
                    .env("KITHARA_PASS_BUDGET_FIXTURE", "1")
                    .status()
                    .unwrap()
                    .success()
            );
            return;
        }
        let started = Instant::now();
        let error = member_output(
            Command::new("sleep").arg("30"),
            None,
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert!(error.to_string().contains("deadline"));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(
            nix::sys::wait::waitpid(
                Pid::from_raw(-1),
                Some(nix::sys::wait::WaitPidFlag::WNOHANG)
            ),
            Err(Errno::ECHILD)
        );

        let started = Instant::now();
        assert!(
            member_output(
                Command::new("sh").args(["-c", "sleep 2 & exit 0"]),
                None,
                Duration::from_millis(200),
            )
            .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_nonreaping_leader_observation_preserves_its_status() {
        let mut child = shell("exit 7");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !leader_exited(&child).unwrap() {
            assert!(Instant::now() < deadline);
            thread::sleep(consts::CHILD_POLL);
        }
        assert!(leader_exited(&child).unwrap());
        assert_eq!(stop_group(&mut child, deadline).unwrap().code(), Some(7));
    }

    #[test]
    fn cancelling_a_pass_control_command_keeps_unrelated_children_alive() {
        let _signals = signals();
        let cancel = Cancel::install().unwrap();
        let pending = Arc::clone(&cancel.pending);
        let mut unrelated = shell("sleep 30");
        let request = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            pending.store(usize::try_from(SIGTERM).unwrap(), Ordering::SeqCst);
        });
        let result = member_output(
            Command::new("sleep").arg("30"),
            Some(&cancel),
            Duration::from_secs(2),
        );
        request.join().unwrap();
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert!(unrelated.try_wait().unwrap().is_none());
        stop(&mut unrelated, "unrelated pass test process").unwrap();
    }

    #[test]
    fn cancellation_stops_the_child_group_and_preserves_an_unrelated_child() {
        let mut unrelated = shell("sleep 30");
        let _signals = signals();
        let cancel = Cancel::install().unwrap();
        let mut owned = spawn(
            Command::new("sh")
                .args(["-c", "sleep 30 & echo ready; wait"])
                .stdout(Stdio::piped()),
        )
        .unwrap();
        let mut stdout = std::io::BufReader::new(owned.stdout.take().unwrap());
        let mut ready = String::new();
        std::io::BufRead::read_line(&mut stdout, &mut ready).unwrap();
        assert_eq!(ready, "ready\n");
        for signal in [SIGINT, SIGTERM] {
            let expected = usize::try_from(signal).unwrap();
            low_level::raise(signal).unwrap();
            assert_eq!(cancel.pending.load(Ordering::SeqCst), expected);
        }
        let error = supervise(&mut owned, Some(&cancel), None).unwrap_err();
        assert!(error.to_string().contains("cancelled"));
        assert!(owned.try_wait().unwrap().is_some());
        let (closed, receive) = mpsc::channel();
        thread::spawn(move || {
            let mut bytes = Vec::new();
            closed.send(stdout.read_to_end(&mut bytes)).unwrap();
        });
        receive
            .recv_timeout(Duration::from_secs(2))
            .expect("descendant still holds stdout")
            .unwrap();
        assert!(unrelated.try_wait().unwrap().is_none());
        stop(&mut unrelated, "unrelated test process").unwrap();
    }
}
