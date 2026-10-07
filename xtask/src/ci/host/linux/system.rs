use anyhow::{Context, Result, bail};
use tracing::info;

use super::profile::LinuxHost;
use crate::{
    ci::{config::CiPins, process::Process},
    consts,
};

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

    let pins = CiPins::load(std::path::Path::new(consts::PINS_PATH))?;
    for (volume, image) in &job_mounts(host, &pins)? {
        if std::path::Path::new(volume).is_absolute() {
            std::fs::create_dir_all(volume)
                .with_context(|| format!("creating runner cache directory {volume}"))?;
        } else {
            process.run(
                "docker",
                &["volume", "create", volume],
                "create a runner cache volume",
            )?;
        }
        give_to_the_job(process, volume, image)?;
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
    let missing: Vec<&str> = consts::HOST_PACKAGES
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

/// Every cache mount the runners use, each named once with the image of a
/// runner that mounts it: the machine holds that image under the tag its unit
/// names, while a pin is only what the next image build tags.
fn job_mounts(host: &LinuxHost, pins: &CiPins) -> Result<Vec<(String, String)>> {
    let mut mounts: Vec<(String, String)> = Vec::new();
    for runner in &host.runners {
        let image = super::container::image(runner, pins)?;
        for (volume, _) in super::container::Container::mounts(host, runner) {
            if !mounts.iter().any(|(named, _)| *named == volume) {
                mounts.push((volume, image.clone()));
            }
        }
    }
    mounts.sort();
    Ok(mounts)
}

fn require_linux() -> Result<()> {
    if !cfg!(target_os = "linux") {
        bail!("this command provisions a Linux CI machine and must run on one");
    }
    Ok(())
}

/// Hand a cache mount to the user the job runs as.
///
/// Docker fills a fresh named volume from the image, ownership included — but
/// only where the image has that directory. A mount point the image does not
/// carry gets an empty volume owned by root, and a job that is not root then
/// cannot write to its own cache. That is not a cache being temperamental: it
/// is a volume nobody gave away. Both existing volumes worked by accident of
/// their paths existing in the image; this makes it true on purpose, for every
/// cache mount, on every machine that bootstraps.
fn give_to_the_job(process: &Process, volume: &str, image: &str) -> Result<()> {
    let mount_type = super::container::Container::mount_type(volume);
    let mount = format!("type={mount_type},source={volume},target=/volume");
    let owner = format!("chown {user}:{user} /volume", user = consts::JOB_USER);
    process.run(
        "docker",
        &[
            "run",
            "--rm",
            // As root, because the point is to give the directory away, and the
            // image starts as the user being given it.
            "--user",
            "0:0",
            "--mount",
            &mount,
            "--entrypoint",
            "sh",
            image,
            "-c",
            &owner,
        ],
        "give a cache volume to the job user",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ci::{
        config::fixture,
        host::linux::{container, profile::tests::host_fixture},
    };

    /// A machine holds the images its units start from, under the tag the
    /// units name; a pin is what the next image build tags. Handing a mount
    /// over through the pin fails on a machine that serves every job.
    #[test]
    fn every_cache_mount_is_handed_over_through_an_image_a_unit_starts_from() {
        let host = host_fixture();
        let pins = &fixture().pins;
        let started: Vec<String> = host
            .runners
            .iter()
            .map(|runner| container::image(runner, pins).expect("the pins carry tags"))
            .collect();
        let mounts = job_mounts(&host, pins).expect("the pins carry tags");
        assert!(!mounts.is_empty(), "the runners mount caches");
        for (volume, image) in &mounts {
            assert!(
                started.contains(image),
                "{volume} goes through {image}, which no unit starts from: {started:?}"
            );
        }
    }
}
