use std::{fmt::Write as _, path::PathBuf};

use anyhow::{Context, Result};
use tracing::info;

use super::profile::{LINUX_CONFIG_PATH, LinuxHost, LinuxRunner, RunnerFlavor};
use crate::ci::{config::CiPins, process::Process};

/// Where the services live and what they call.
///
/// The executable is copied out of the build tree: run from there it expects
/// to find the repository around it, and a service starting from no particular
/// directory would not.
struct Layout {
    systemd_root: &'static str,
    executable: &'static str,
}

const LAYOUT: Layout = Layout {
    systemd_root: "/etc/systemd/system",
    executable: "/usr/local/bin/kithara-ci",
};

/// Write one service per runner and hand them to systemd.
///
/// Each service configures its runner just before starting it, so a machine
/// that has been off for a week still comes back with credentials that were
/// minted seconds ago rather than ones that expired while it slept.
pub(super) fn install(
    process: &Process,
    host: &LinuxHost,
    pins: &CiPins,
    executable: &str,
) -> Result<()> {
    std::fs::copy(executable, LAYOUT.executable)
        .with_context(|| format!("installing {}", LAYOUT.executable))?;
    std::fs::set_permissions(
        LAYOUT.executable,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .with_context(|| format!("making {} executable", LAYOUT.executable))?;

    for runner in &host.runners {
        let path = PathBuf::from(LAYOUT.systemd_root).join(runner.service());
        std::fs::write(&path, unit(host, runner, pins, LAYOUT.executable)?)
            .with_context(|| format!("writing {}", path.display()))?;
        info!(service = runner.service(), "runner service installed");
    }
    process.run("systemctl", &["daemon-reload"], "reload systemd")?;
    for runner in &host.runners {
        process.run(
            "systemctl",
            &["enable", "--now", &runner.service()],
            "enable runner service",
        )?;
    }
    Ok(())
}

/// The runner takes one job and exits, the container goes with it, and systemd
/// starts the next one. Nothing survives a job except the caches.
fn unit(host: &LinuxHost, runner: &LinuxRunner, pins: &CiPins, executable: &str) -> Result<String> {
    let mut unit = String::new();
    writeln!(
        unit,
        "[Unit]\n\
         Description=Kithara CI runner {name} (GitHub Actions, ephemeral)\n\
         After=docker.service\n\
         Requires=docker.service\n\n\
         [Service]\n\
         Type=simple\n\
         Restart=always\n\
         RestartSec=10\n\
         RuntimeDirectory=kithara-ci\n\
         RuntimeDirectoryMode=0700\n\n\
         ExecStartPre={executable} ci linux firewall --config {config}\n\
         ExecStartPre={executable} ci linux configure --config {config} --runner {name} \
         --env-file {env_file}\n",
        name = runner.name,
        config = LINUX_CONFIG_PATH,
        env_file = env_file(runner),
    )?;

    write!(
        unit,
        "\nExecStart=/usr/bin/docker run --rm --name kithara-ci-{name} \
         --network {network} \
         --cpus {cpus} \
         --memory {memory} \
         --pids-limit 8192 \
         --security-opt no-new-privileges \
         --env-file {env_file} \
         --env CARGO_TARGET_DIR=/cache/target \
         --mount type=volume,source=kithara-ci-cargo-registry,target=/usr/local/cargo/registry \
         --mount type=volume,source=kithara-ci-cargo-git,target=/usr/local/cargo/git \
         --mount type=volume,source=kithara-ci-target,target=/cache/target",
        name = runner.name,
        network = host.network,
        cpus = runner.cpus,
        memory = runner.memory,
        env_file = env_file(runner),
    )?;
    if runner.gpus {
        write!(unit, " --gpus all --env NVIDIA_DRIVER_CAPABILITIES=all")?;
    }
    for device in &runner.devices {
        write!(unit, " --device {}", device.display())?;
    }
    let image = match runner.flavor {
        RunnerFlavor::Plain => &pins.linux_runner_image,
        RunnerFlavor::Android => &pins.linux_android_runner_image,
    };
    writeln!(unit, " {image}")?;

    writeln!(
        unit,
        "\nExecStopPost=-/usr/bin/docker rm -f kithara-ci-{name}\n\n\
         [Install]\n\
         WantedBy=multi-user.target",
        name = runner.name,
    )?;
    Ok(unit)
}

/// systemd creates the runtime directory before the first `ExecStartPre`, so
/// the configuration lands somewhere that is wiped when the service stops.
pub(super) fn env_file(runner: &LinuxRunner) -> String {
    format!("/run/kithara-ci/{}.env", runner.name)
}

/// Report what the machine is serving. A runner that is up but unregistered
/// looks identical to a healthy one from the host's side, so the report names
/// the service state rather than claiming a verdict it cannot reach.
pub(super) fn health(process: &Process, host: &LinuxHost) -> Result<()> {
    // Without this, a missing systemctl would read as every runner being down.
    process.require_tools(&["systemctl"])?;
    for runner in &host.runners {
        let state = process
            .capture(
                "systemctl",
                &["is-active", &runner.service()],
                "read a runner service state",
            )
            .unwrap_or_else(|_| "inactive".to_owned());
        info!(
            runner = runner.name,
            labels = runner.labels(),
            state,
            "runner service"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ci::{config::fixture, linux::profile::tests::host_fixture};

    #[test]
    fn a_gpu_runner_reaches_the_devices_a_plain_one_does_not() {
        let host = host_fixture();
        let pins = &fixture().pins;
        let plain = unit(
            &host,
            host.runner("kithara-ci").unwrap(),
            pins,
            "/usr/bin/xtask",
        )
        .unwrap();
        let gpu = unit(
            &host,
            host.runner("kithara-ci-gpu").unwrap(),
            pins,
            "/usr/bin/xtask",
        )
        .unwrap();

        let android = unit(
            &host,
            host.runner("kithara-ci-android").unwrap(),
            pins,
            "/usr/bin/xtask",
        )
        .unwrap();

        assert!(!plain.contains("--gpus"), "{plain}");
        assert!(!plain.contains("--device"), "{plain}");
        assert!(gpu.contains("--gpus all"), "{gpu}");
        assert!(android.contains("--device /dev/kvm"), "{android}");
        assert!(
            android.contains(pins.linux_android_runner_image.as_str()),
            "{android}"
        );
        for unit in [&plain, &gpu, &android] {
            assert!(unit.contains("--security-opt no-new-privileges"), "{unit}");
            assert!(!unit.contains("docker.sock"), "{unit}");
        }
    }
}
