use anyhow::{Context, Result, bail};
use tracing::info;

use super::profile::LinuxHost;
use crate::ci::process::Process;

/// Host packages a runner machine needs beyond Docker itself. Everything a job
/// compiles with lives in the image; these are the pieces that must sit on the
/// host because they reach hardware.
///
/// Most of them are what a Windows guest costs: the daemon that owns it, the
/// resolver its network cannot start without, the tool that creates its disk,
/// firmware it can boot from, a
/// software TPM it refuses to install without, the tool that creates it, and
/// one to build the answer file that installs it unattended.
const HOST_PACKAGES: [&str; 10] = [
    "iptables",
    "dnsmasq-base",
    "qemu-utils",
    "nvidia-container-toolkit",
    "qemu-system-x86",
    "libvirt-daemon-system",
    "ovmf",
    "swtpm-tools",
    "virtinst",
    "xorriso",
];

/// Prepare the machine a runner will live on: its caches, its network, and the
/// packages that cannot live in an image.
pub(super) fn bootstrap(process: &Process, host: &LinuxHost) -> Result<()> {
    require_linux()?;
    process.require_tools(&["docker"])?;

    std::fs::create_dir_all(&host.cache_root)
        .with_context(|| format!("creating {}", host.cache_root.display()))?;

    // Docker refuses to create a network that exists, and refusing to continue
    // over that would make every later run of this command fail.
    let existing = process.capture(
        "docker",
        &["network", "ls", "--format", "{{.Name}}"],
        "list Docker networks",
    )?;
    if !existing.lines().any(|name| name == host.network) {
        process.run(
            "docker",
            &["network", "create", "--subnet", &host.subnet, &host.network],
            "create the runner network",
        )?;
    }

    for (volume, _) in super::container::Container::MOUNTS {
        process.run(
            "docker",
            &["volume", "create", volume],
            "create a runner cache volume",
        )?;
    }
    info!(network = host.network, "runner machine prepared");
    Ok(())
}

/// Install the host packages. GPU access needs the container toolkit and an
/// emulator needs QEMU, and neither can be carried in the image that uses them.
pub(super) fn install_tools(process: &Process) -> Result<()> {
    require_linux()?;

    // Only what is missing. Naming a package that is already installed invites
    // apt to upgrade it, and upgrading the GPU stack underneath a machine that
    // is serving other work is not this command's business.
    let missing: Vec<&str> = HOST_PACKAGES
        .into_iter()
        .filter(|package| {
            // A package dpkg cannot describe at all is missing just as surely
            // as one it describes as not installed.
            process
                .capture(
                    "dpkg-query",
                    &["-W", "-f=${Status}", package],
                    "read a package's state",
                )
                .ok()
                .is_none_or(|status| !status.starts_with("install ok installed"))
        })
        .collect();
    if missing.is_empty() {
        info!("host packages already present");
        return Ok(());
    }
    info!(packages = missing.join(", "), "installing host packages");

    process.run("apt-get", &["update"], "refresh the package index")?;
    let mut install = process.command("apt-get");
    install
        .args(["install", "-y", "--no-install-recommends"])
        .args(&missing);
    process.run_command(&mut install, "install host packages")?;

    // The toolkit ships the runtime but does not register it, and a GPU runner
    // that starts without it fails on its first job rather than at setup.
    //
    // Only when it was this command that installed it: restarting Docker stops
    // every container on the machine, including ones this repository does not
    // own, and doing that to re-apply a configuration that is already in place
    // would be a poor trade.
    if missing.contains(&"nvidia-container-toolkit") {
        process.run(
            "nvidia-ctk",
            &["runtime", "configure", "--runtime=docker"],
            "register the GPU container runtime",
        )?;
        process.run("systemctl", &["restart", "docker"], "restart Docker")?;
    }
    Ok(())
}

fn require_linux() -> Result<()> {
    if !cfg!(target_os = "linux") {
        bail!("this command provisions a Linux CI machine and must run on one");
    }
    Ok(())
}
