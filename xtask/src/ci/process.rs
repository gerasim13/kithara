use std::{
    collections::BTreeMap,
    env,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context, Result, bail};
use tracing::{info, warn};

pub(crate) struct Process {
    root: PathBuf,
    vars: BTreeMap<OsString, OsString>,
}

impl Process {
    pub(crate) fn new(root: &Path, vars: BTreeMap<OsString, OsString>) -> Self {
        Self {
            root: root.to_path_buf(),
            vars,
        }
    }

    pub(crate) fn command(&self, program: impl AsRef<OsStr>) -> Command {
        self.command_in(program, "")
    }

    /// A command that runs inside a subdirectory of the checkout. Build tools
    /// that locate their project by walking up from the working directory —
    /// Gradle looks for the settings file — need the directory that owns them,
    /// not the workspace root.
    pub(crate) fn command_in(&self, program: impl AsRef<OsStr>, relative: &str) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(self.root.join(relative))
            .envs(&self.vars);
        command
    }

    pub(crate) fn run(&self, program: &str, args: &[&str], label: &str) -> Result<()> {
        let mut command = self.command(program);
        command.args(args);
        self.run_command(&mut command, label)
    }

    pub(crate) fn run_command(&self, command: &mut Command, label: &str) -> Result<()> {
        info!(step = label, root = %self.root.display(), "starting");
        let status = command
            .status()
            .with_context(|| format!("failed to start {label}"))?;
        if !status.success() {
            bail!(
                "{label} failed with exit code {}",
                status.code().unwrap_or(-1)
            );
        }
        Ok(())
    }

    pub(crate) fn capture(&self, program: &str, args: &[&str], label: &str) -> Result<String> {
        let output = self
            .command(program)
            .args(args)
            .output()
            .with_context(|| format!("failed to start {label}"))?;
        output_text(output, label)
    }

    pub(crate) fn best_effort(&self, program: &str, args: &[&str], label: &str) {
        if let Err(error) = self.run(program, args, label) {
            warn!(step = label, %error, "best-effort step failed");
        }
    }

    pub(crate) fn require_tools(&self, tools: &[&str]) -> Result<()> {
        for tool in tools {
            if self.find_executable(tool).is_none() {
                bail!("required CI command is missing: {tool}");
            }
        }
        Ok(())
    }

    fn find_executable(&self, program: &str) -> Option<PathBuf> {
        let path = Path::new(program);
        if path.components().count() > 1 {
            return path.is_file().then(|| path.to_path_buf());
        }

        let search = self
            .vars
            .get(OsStr::new("PATH"))
            .cloned()
            .or_else(|| env::var_os("PATH"))?;
        let extensions = executable_extensions(&self.vars);
        env::split_paths(&search).find_map(|directory| {
            extensions.iter().find_map(|extension| {
                let candidate = directory.join(format!("{program}{extension}"));
                candidate.is_file().then_some(candidate)
            })
        })
    }
}

pub(crate) fn require_os(expected: &str, label: &str) -> Result<()> {
    if env::consts::OS != expected {
        bail!(
            "{label} lane requires {expected}, current platform is {}",
            env::consts::OS
        );
    }
    Ok(())
}

fn executable_extensions(vars: &BTreeMap<OsString, OsString>) -> Vec<String> {
    if cfg!(windows) {
        vars.get(OsStr::new("PATHEXT"))
            .cloned()
            .or_else(|| env::var_os("PATHEXT"))
            .map_or_else(
                || vec![".exe".into(), ".cmd".into(), ".bat".into()],
                |value| {
                    value
                        .to_string_lossy()
                        .split(';')
                        .map(str::to_ascii_lowercase)
                        .collect()
                },
            )
    } else {
        vec![String::new()]
    }
}

fn output_text(output: Output, label: &str) -> Result<String> {
    if !output.status.success() {
        bail!(
            "{label} failed with exit code {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout)
        .with_context(|| format!("{label} produced non-UTF-8 standard output"))
        .map(|text| text.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_output_keeps_command_context() {
        let output = if cfg!(windows) {
            Command::new("cmd")
                .args(["/C", "exit", "7"])
                .output()
                .unwrap()
        } else {
            Command::new("sh").args(["-c", "exit 7"]).output().unwrap()
        };
        let error = output_text(output, "fixture command").unwrap_err();
        assert!(error.to_string().contains("fixture command"));
        assert!(error.to_string().contains('7'));
    }
}
