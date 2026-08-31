use std::path::PathBuf;

use super::profile::{LinuxHost, LinuxRunner, RunnerFlavor};
use crate::ci::{LINUX_LINKER_ENV, config::CiPins};

/// Where a job builds and what it reuses, before the linker entries are added.
const CACHE_ENVIRONMENT: [&str; 6] = [
    // Encoded audio fixtures. Their default home is the container's own temp
    // directory, and a container serves one job and is thrown away — so every
    // job re-encoded every fixture it touched, and a test that builds one
    // inside its own deadline lost the race under load. Entries are
    // content-addressed and namespaced by a build fingerprint, so sharing them
    // across runners cannot serve one build's bytes to another.
    "KITHARA_FIXTURE_CACHE=/cache/fixtures",
    "CARGO_TARGET_DIR=/cache/target",
    "RUSTC_WRAPPER=sccache",
    // Without this the wrapper is inert: sccache declines to cache an
    // incremental compilation, and cargo leaves incremental on by default.
    // Setting the wrapper and not this is how a cache gets installed, enabled,
    // and still never hit.
    "CARGO_INCREMENTAL=0",
    "SCCACHE_DIR=/cache/sccache",
    // Well under the volume it lives on, and sccache evicts by least use
    // rather than growing until the disk decides for it.
    "SCCACHE_CACHE_SIZE=100G",
];

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
    /// Volumes this runner mounts, in the order the job sees them.
    pub(super) mounts: Vec<(String, &'static str)>,
}

impl Container<'_> {
    /// The cargo home is mounted whole rather than as its registry and its git
    /// checkouts separately: cargo guards both with a lock file kept beside
    /// them, and jobs on this machine run at the same time. Mounting the data
    /// without the lock leaves two of them unpacking one crate into one
    /// directory.
    /// The volumes every runner shares, and the one it keeps to itself.
    ///
    /// The registry of downloaded crates is shared because that is what it is
    /// for, and the compiler cache because `sccache` keys on the inputs of a
    /// compilation, so one runner's entry is another's hit.
    ///
    /// The build directory is not. Its artefacts are valid only for the exact
    /// features, profile and toolchain that produced them, so runners of
    /// different shapes reuse none of each other's and only contend for the
    /// same directory — which is how one grew past two hundred gigabytes while
    /// every job still compiled from source. Each runner keeps its own and
    /// warms it with its own repeat work.
    pub(super) fn mounts(runner: &LinuxRunner) -> Vec<(String, &'static str)> {
        vec![
            ("kithara-ci-cargo-home".to_owned(), "/home/runner/.cargo"),
            (
                format!("kithara-ci-target-{}", runner.name),
                "/cache/target",
            ),
            ("kithara-ci-sccache".to_owned(), "/cache/sccache"),
            ("kithara-ci-fixtures".to_owned(), "/cache/fixtures"),
        ]
    }

    /// What the job is told about where to build and what to reuse.
    ///
    /// `sccache` is in the image and was reaching nothing: without
    /// `RUSTC_WRAPPER` every job compiled the workspace from source, and the
    /// only thing the runners shared was the registry of downloaded crates and
    /// one build directory that had grown past two hundred gigabytes. A build
    /// directory is the wrong thing to share — its artefacts are valid only for
    /// the exact features, profile and toolchain that produced them, so
    /// twenty-four jobs of different shapes pile up beside each other and reuse
    /// nothing.
    /// `sccache` keys on the inputs of a compilation instead, which is what
    /// makes sharing it across runners sound rather than merely concurrent.
    ///
    /// The linker entries come from [`LINUX_LINKER_ENV`], which the GitLab lane
    /// executor reads too: one statement of what a Linux job links with rather
    /// than one per way of starting a job.
    pub(super) fn environment() -> Vec<String> {
        let mut environment: Vec<String> = CACHE_ENVIRONMENT
            .iter()
            .map(|entry| (*entry).to_owned())
            .collect();
        environment.extend(
            LINUX_LINKER_ENV
                .iter()
                .map(|(name, value)| format!("{name}={value}")),
        );
        environment
    }

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
        mounts: Container::mounts(runner),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The linker a Linux job links with is part of what a job is told, not a
    /// property of whichever image happened to be built: an unnamed linker is
    /// `bfd`, and `bfd` is where a test job spends more time than it spends
    /// testing.
    #[test]
    fn a_job_is_told_which_linker_to_use() {
        let environment = Container::environment();

        for (name, value) in LINUX_LINKER_ENV {
            assert!(
                environment.contains(&format!("{name}={value}")),
                "{name} is missing from {environment:?}"
            );
        }
    }
}
