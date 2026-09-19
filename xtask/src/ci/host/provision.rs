use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::ci::process::Process;

/// What the provisioning pass is told about the machine it is on.
pub(super) struct Provision<'a> {
    pub(super) process: &'a Process,
    /// Machine profile, as this platform's command spells it.
    pub(super) config: &'a Path,
    /// Reviewed build pins tracked in the repository.
    pub(super) pins: &'a Path,
}

impl Provision<'_> {
    /// Run one of this host's own subcommands again, as root.
    ///
    /// The provisioning job is served by the unprivileged runner user, so the
    /// steps that own root-owned files — the installed executable, the service
    /// definitions — cannot run in-process. `sudo -n` never prompts: a machine
    /// that has not granted this fails the step with what to grant instead of
    /// hanging the pipeline on a password nobody is there to type.
    pub(super) fn as_root(&self, platform: &str, step: &str) -> Result<()> {
        let executable = std::env::current_exe().context("locating this executable")?;
        let executable = executable
            .to_str()
            .context("this executable's path is not UTF-8")?;
        let config = path_text(self.config)?;
        let pins = path_text(self.pins)?;
        let arguments = [
            executable, "ci", "host", platform, "--config", config, "--pins", pins, step,
        ];
        if self.process.capture("id", &["-u"], "current user id")? == "0" {
            return self
                .process
                .run(arguments[0], &arguments[1..], &format!("{platform} {step}"));
        }
        let mut command = self.process.command("sudo");
        command.arg("-n").args(arguments);
        self.process
            .run_command(&mut command, &format!("{platform} {step} as root"))
            .with_context(|| {
                format!(
                    "this machine does not let the runner user run `{step}` as root. \
                     Grant exactly that one command, and nothing else, with a line in \
                     /etc/sudoers.d/kithara-ci: \
                     `<runner user> ALL=(root) NOPASSWD: {executable} ci host {platform} *`"
                )
            })
    }
}

fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("path is not UTF-8: {}", path.display()))
}

/// Provision the machine this is running on. A machine only ever provisions
/// itself: the profile, the images and the services are all local, and the
/// platform decides which of them there are.
pub(super) fn run(process: &Process, config: Option<&Path>, pins: &Path) -> Result<()> {
    if cfg!(target_os = "macos") {
        let config = config.context(
            "the macOS host profile lives outside the repository; \
             pass --config or export KITHARA_CI_HOST_CONFIG",
        )?;
        return super::mac::command::provision(&Provision {
            process,
            config,
            pins,
        });
    }
    if cfg!(target_os = "linux") {
        return super::linux::command::provision(process, config, pins);
    }
    bail!("no CI host provisioning for this platform");
}
