use std::path::PathBuf;

use super::profile::{LinuxHost, LinuxRunner, RunnerFlavor};
use crate::ci::config::CiPins;

/// What one runner's container is, independent of who starts it.
///
/// systemd starts these through a `docker run` line and Compose through a
/// service block. Both are renderings of this: a second description would drift
/// from the first, and the drift would be a runner that quietly ran with the
/// wrong cores, the wrong image, or no device.
pub(super) struct Container<'a> {
    pub(super) name: String,
    pub(super) image: &'a str,
    pub(super) network: &'a str,
    /// Which cores its jobs may use, as a set rather than a share. See
    /// [`super::services::cpuset`].
    pub(super) cpuset: String,
    pub(super) memory: &'a str,
    pub(super) devices: &'a [PathBuf],
    pub(super) groups: &'a [u32],
    /// Where the just-in-time registration is left for it. Minted per start and
    /// accepted once, so it is written before the container comes up and is
    /// gone when it stops.
    pub(super) env_file: String,
}

impl Container<'_> {
    /// The cargo home is mounted whole rather than as its registry and its git
    /// checkouts separately: cargo guards both with a lock file kept beside
    /// them, and jobs on this machine run at the same time. Mounting the data
    /// without the lock leaves two of them unpacking one crate into one
    /// directory.
    pub(super) const MOUNTS: [(&'static str, &'static str); 2] = [
        ("kithara-ci-cargo-home", "/home/runner/.cargo"),
        ("kithara-ci-target", "/cache/target"),
    ];
    pub(super) const TARGET_DIR: &'static str = "CARGO_TARGET_DIR=/cache/target";
    pub(super) const PIDS_LIMIT: u32 = 8192;
}

pub(super) fn container<'a>(
    host: &'a LinuxHost,
    runner: &'a LinuxRunner,
    cpuset: String,
    pins: &'a CiPins,
) -> Container<'a> {
    Container {
        name: format!("kithara-ci-{}", runner.name),
        image: match runner.flavor {
            RunnerFlavor::Plain => &pins.linux_runner_image,
            RunnerFlavor::Android => &pins.linux_android_runner_image,
        },
        network: &host.network,
        cpuset,
        memory: &runner.memory,
        devices: &runner.devices,
        groups: &runner.groups,
        env_file: super::services::env_file(runner),
    }
}
