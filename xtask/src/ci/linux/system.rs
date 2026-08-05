use anyhow::{Context, Result, bail};
use tracing::info;

use super::profile::LinuxHost;
use crate::ci::process::Process;

/// Host packages a runner machine needs beyond Docker itself. Everything a job
/// compiles with lives in the image; these are the pieces that must sit on the
/// host because they reach hardware.
const HOST_PACKAGES: [&str; 3] = ["iptables", "nvidia-container-toolkit", "qemu-system-x86"];

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

    for volume in [
        "kithara-ci-cargo-registry",
        "kithara-ci-cargo-git",
        "kithara-ci-target",
    ] {
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
    process.run("apt-get", &["update"], "refresh the package index")?;
    let mut install = process.command("apt-get");
    install
        .args(["install", "-y", "--no-install-recommends"])
        .args(HOST_PACKAGES);
    process.run_command(&mut install, "install host packages")?;

    // The toolkit ships the runtime but does not register it, and a GPU runner
    // that starts without it fails on its first job rather than at setup.
    process.run(
        "nvidia-ctk",
        &["runtime", "configure", "--runtime=docker"],
        "register the GPU container runtime",
    )?;
    process.run("systemctl", &["restart", "docker"], "restart Docker")
}

fn require_linux() -> Result<()> {
    if !cfg!(target_os = "linux") {
        bail!("this command provisions a Linux CI machine and must run on one");
    }
    Ok(())
}
