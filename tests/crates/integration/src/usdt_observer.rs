//! External DTrace observer for the USDT provider acceptance lane.
//!
//! This lives entirely outside the instrumented process. It only consumes the
//! textual records emitted by DTrace, so it cannot become a test-only product
//! observation path.

use std::{
    ffi::OsString,
    io::{BufRead, BufReader},
    process::{Child, Command, Output, Stdio},
    sync::mpsc,
};

use anyhow::{Context, Result, bail};
use kithara::platform::time::Duration;
use tempfile::NamedTempFile;

const RECORD_PREFIX: &str = "KITHARA_USDT";
const RECORD_PREFIX_WITH_SEPARATOR: &str = "KITHARA_USDT|";
const READY_MARKER: &str = "KITHARA_USDT_READY";
const DTRACE: &str = "/usr/sbin/dtrace";
const SUDO: &str = "sudo";
const DEFAULT_OBSERVATION_WINDOW: Duration = Duration::from_secs(2);
const MAX_OBSERVATION_WINDOW: Duration = Duration::from_secs(15 * 60);

/// One raw provider firing as observed outside the instrumented process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeRecord {
    pub arity: u8,
    pub operation: u64,
    pub payload: [u64; 5],
}

/// A fully specified observer command, exposed for deterministic tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObserverCommand {
    pub program: &'static str,
    pub args: Vec<OsString>,
}

/// A running external observer with a bounded capture window.
pub struct Observer {
    child: Child,
    output: NamedTempFile,
}

impl Observer {
    /// Waits for the bounded DTrace capture and parses every raw provider record.
    ///
    /// The observer program exits after its configured capture window,
    /// independently of its target. This permits a test to observe its own
    /// process and call `collect` without waiting for that process to exit.
    pub fn collect(self) -> Result<Vec<ProbeRecord>> {
        let output = self.child.wait_with_output()?;
        assert_observer_succeeded(&output)?;
        let records = std::fs::read_to_string(self.output.path())
            .context("read external DTrace observer output")?;
        parse_records(&records)
    }
}

impl ObserverCommand {
    fn spawn(&self) -> Result<Child> {
        Command::new(self.program)
            .args(&self.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("start external USDT observer {}", self.program))
    }
}

/// Builds the passwordless DTrace command that observes one process.
pub fn observer_command(pid: u32) -> Result<ObserverCommand> {
    observer_command_for(pid, DEFAULT_OBSERVATION_WINDOW)
}

/// Builds the DTrace command for a bounded capture window.
pub fn observer_command_for(pid: u32, window: Duration) -> Result<ObserverCommand> {
    Ok(ObserverCommand {
        program: SUDO,
        args: vec![
            OsString::from("-n"),
            OsString::from(DTRACE),
            OsString::from("-q"),
            OsString::from("-n"),
            OsString::from(dtrace_program(window)?),
            OsString::from("-p"),
            OsString::from(pid.to_string()),
        ],
    })
}

/// Builds an attach-only capability preflight for a same-user process.
///
/// This must not name `kithara` probes: the disposable child does not contain
/// that provider, so enabling it would test the wrong condition.
#[must_use]
pub fn preflight_command(pid: u32) -> ObserverCommand {
    ObserverCommand {
        program: SUDO,
        args: vec![
            OsString::from("-n"),
            OsString::from(DTRACE),
            OsString::from("-q"),
            OsString::from("-n"),
            OsString::from("BEGIN { exit(0); }"),
            OsString::from("-p"),
            OsString::from(pid.to_string()),
        ],
    }
}

/// Starts an observer using the short default capture window.
pub fn observe(pid: u32) -> Result<Observer> {
    observe_for(pid, DEFAULT_OBSERVATION_WINDOW)
}

/// Starts an observer for a bounded capture window.
///
/// The observer waits for DTrace's `BEGIN` marker before returning, so a
/// caller can fire a real operation immediately afterwards without racing
/// probe registration. The observer exits after `window`, not when `pid`
/// exits; choose a window that covers the product action under test.
pub fn observe_for(pid: u32, window: Duration) -> Result<Observer> {
    let output = NamedTempFile::new().context("create external DTrace output file")?;
    let mut command = observer_command_for(pid, window)?;
    command.args[4] = OsString::from(dtrace_program_to_file(window, output.path())?);
    let mut child = command.spawn()?;
    if let Err(readiness) = wait_until_ready(&mut child, window) {
        return Err(match cleanup_observer(child) {
            Ok(output) => readiness.context(format!(
                "DTrace observer exited after readiness failure: status={} stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(cleanup) => readiness.context(format!(
                "DTrace observer cleanup failed after readiness failure: {cleanup:#}"
            )),
        });
    }
    Ok(Observer { child, output })
}

/// Proves that this runner can attach DTrace to a same-user process.
///
/// The production proof cannot accept a runner that merely has the binary:
/// SIP policy and sudoers entitlement are enforced at attach time. The child
/// is intentionally ordinary and same-user, matching the test process that
/// the observer will attach to in the acceptance suite.
pub fn preflight() -> Result<()> {
    let mut target = Command::new("/bin/sleep")
        .arg("2")
        .spawn()
        .context("start same-user DTrace preflight child")?;
    let observer = preflight_command(target.id()).spawn()?;
    let target_status = target.wait().context("wait for DTrace preflight child")?;
    if !target_status.success() {
        bail!("same-user DTrace preflight child exited with {target_status}");
    }
    assert_observer_succeeded(&observer.wait_with_output()?)?;
    Ok(())
}

/// Parses all provider records in DTrace stdout.
pub fn parse_records(output: &str) -> Result<Vec<ProbeRecord>> {
    output
        .lines()
        .filter(|line| line.starts_with(RECORD_PREFIX_WITH_SEPARATOR))
        .map(parse_record)
        .collect()
}

fn parse_record(line: &str) -> Result<ProbeRecord> {
    let mut fields = line.trim().split('|');
    let prefix = fields.next();
    if prefix != Some(RECORD_PREFIX) {
        bail!("not a Kithara USDT record: {line}");
    }
    let probe = fields.next().context("USDT record has no probe name")?;
    let arity = probe
        .strip_prefix("probe_")
        .context("USDT record has an unknown probe name")?
        .parse::<u8>()
        .context("USDT probe arity is not an integer")?;
    if arity > 5 {
        bail!("USDT provider reported unsupported probe arity {arity}");
    }
    let values = fields
        .map(|value| value.parse::<u64>().context("USDT argument is not a u64"))
        .collect::<Result<Vec<_>>>()?;
    let expected = usize::from(arity) + 1;
    if values.len() != expected {
        bail!(
            "USDT {probe} has {} arguments; expected operation id plus {arity} payload values",
            values.len()
        );
    }
    let operation = values[0];
    let mut payload = [0; 5];
    payload[..usize::from(arity)].copy_from_slice(&values[1..]);
    Ok(ProbeRecord {
        arity,
        operation,
        payload,
    })
}

fn assert_observer_succeeded(output: &Output) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "DTrace cannot attach to a same-user process: status={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
}

fn wait_until_ready(child: &mut Child, window: Duration) -> Result<()> {
    let stdout = child
        .stdout
        .take()
        .context("DTrace observer stdout is unavailable")?;
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let line = BufReader::new(stdout).lines().next().transpose();
        let _ = sender.send(line);
    });
    match receiver.recv_timeout(window) {
        Ok(Ok(Some(line))) if line == READY_MARKER => Ok(()),
        Ok(Ok(Some(line))) => {
            bail!("DTrace observer emitted unexpected readiness line: {line}")
        }
        Ok(Ok(None)) => bail!("DTrace observer readiness stream closed before BEGIN"),
        Ok(Err(error)) => Err(error).context("read DTrace observer readiness stream"),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            bail!("DTrace observer did not become ready within {window:?}")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            bail!("DTrace observer readiness stream closed before BEGIN")
        }
    }
}

fn cleanup_observer(mut child: Child) -> Result<Output> {
    let kill_error = match child
        .try_wait()
        .context("poll DTrace observer after readiness failure")?
    {
        Some(_) => None,
        None => child.kill().err(),
    };
    let output = child
        .wait_with_output()
        .context("reap DTrace observer after readiness failure")?;
    if let Some(kill_error) = kill_error {
        return Err(anyhow::Error::new(kill_error).context(format!(
            "stop DTrace observer after readiness failure: status={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output)
}

fn dtrace_program_to_file(window: Duration, output: &std::path::Path) -> Result<String> {
    let destination = output.to_string_lossy();
    if destination.contains('"') {
        bail!("DTrace output path contains an unsupported quote");
    }
    Ok((0_u8..=5)
        .map(|arity| dtrace_file_clause(arity, &destination))
        .chain(std::iter::once(format!(
            "BEGIN {{ printf(\"{READY_MARKER}\\n\"); }}"
        )))
        .chain(std::iter::once(tick_clause(window)?))
        .collect::<Vec<_>>()
        .join(" "))
}

fn dtrace_program(window: Duration) -> Result<String> {
    let seconds = window.as_secs();
    if seconds == 0 || window.subsec_nanos() != 0 {
        bail!("USDT observation window must be a positive whole number of seconds");
    }
    if window > MAX_OBSERVATION_WINDOW {
        bail!("USDT observation window exceeds {MAX_OBSERVATION_WINDOW:?}");
    }
    Ok((0_u8..=5)
        .map(dtrace_clause)
        .chain(std::iter::once(format!(
            "BEGIN {{ printf(\"{READY_MARKER}\\n\"); }}"
        )))
        .chain(std::iter::once(format!("tick-{seconds}sec {{ exit(0); }}")))
        .collect::<Vec<_>>()
        .join(" "))
}

fn tick_clause(window: Duration) -> Result<String> {
    let seconds = window.as_secs();
    if seconds == 0 || window.subsec_nanos() != 0 || window > MAX_OBSERVATION_WINDOW {
        bail!("USDT observation window is invalid");
    }
    Ok(format!("tick-{seconds}sec {{ exit(0); }}"))
}

fn dtrace_file_clause(arity: u8, output: &str) -> String {
    dtrace_clause(arity).replacen("printf(", &format!("fprintf(\"{output}\", "), 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[kithara::test(native, flash(false))]
    fn streamed_readiness_keeps_records_out_of_stdout() {
        let program = dtrace_program_to_file(
            Duration::from_secs(2),
            std::path::Path::new("/tmp/kithara-usdt-records"),
        )
        .expect("fixed test path and window are valid");
        assert!(program.contains("fprintf(\"/tmp/kithara-usdt-records\", \"KITHARA_USDT|probe_0"));
        assert!(!program.contains("printf(\"KITHARA_USDT|probe_"));
        assert_eq!(program.matches("BEGIN {").count(), 1);
        assert!(program.contains("BEGIN { printf(\"KITHARA_USDT_READY\\n\"); }"));
    }

    #[kithara::test(native, flash(false))]
    fn cleanup_observer_reaps_a_running_child() {
        let child = Command::new("/bin/sh")
            .args(["-c", "sleep 60"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start child");

        let output = cleanup_observer(child).expect("cleanup child");

        assert!(!output.status.success());
    }
}

fn dtrace_clause(arity: u8) -> String {
    let fields = (0..=arity).map(|_| "|%llu").collect::<String>();
    let arguments = (0..=arity)
        .map(|index| format!("arg{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "kithara:::probe_{arity} {{ printf(\"{RECORD_PREFIX}|probe_{arity}{fields}\\n\", {arguments}); }}"
    )
}
