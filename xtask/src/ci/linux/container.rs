use std::path::{Path, PathBuf};

use super::profile::{LinuxHost, LinuxRunner, RunnerFlavor};
use crate::ci::{LINUX_LINKER_ENV, config::CiPins};

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
    /// Where a job builds and what it reuses, before the linker entries are added.
    const CACHE_ENVIRONMENT: [&'static str; 5] = [
        // Encoded audio fixtures. Their default home is the container's own temp
        // directory, and a container serves one job and is thrown away — so every
        // job re-encoded every fixture it touched, and a test that builds one
        // inside its own deadline lost the race under load. Entries are
        // content-addressed and namespaced by a build fingerprint, so sharing them
        // across runners cannot serve one build's bytes to another.
        "KITHARA_FIXTURE_CACHE=/cache/fixtures",
        "CARGO_TARGET_DIR=/cache/target",
        "CARGO_INCREMENTAL=0",
        "SCCACHE_DIR=/cache/sccache",
        "SCCACHE_CACHE_SIZE=100G",
    ];
    const SCCACHE_ENVIRONMENT: [&'static str; 1] = ["RUSTC_WRAPPER=sccache"];
    const CARGO_REAPI_ENVIRONMENT: [&'static str; 3] = [
        "RUSTC_WRAPPER=kithara-cargo-reapi",
        "CARGO_REAPI_BACKEND=cache",
        "CARGO_REAPI_CACHE_DIR=/cache/cargo-reapi",
    ];

    /// The cargo home is mounted whole rather than as its registry and its git
    /// checkouts separately: cargo guards both with a lock file kept beside
    /// them, and jobs on this machine run at the same time. Mounting the data
    /// without the lock leaves two of them unpacking one crate into one
    /// directory.
    /// The cache mounts every runner shares, and the build directory it keeps
    /// to itself under the host's configured cache root.
    ///
    /// The registry of downloaded crates is shared because that is what it is
    /// for. All runners keep the existing sccache volume so branches predating
    /// cargo-reapi still reuse it. Plain runners additionally share cargo-reapi's
    /// content-addressed compiler and linker outputs.
    ///
    /// The build directory is not shared. Its artefacts are valid only for the exact
    /// features, profile and toolchain that produced them, so runners of
    /// different shapes reuse none of each other's and only contend for the
    /// same directory — which is how one grew past two hundred gigabytes while
    /// every job still compiled from source. Each runner keeps its own and
    /// warms it with its own repeat work. A host path keeps that write-heavy
    /// cache on the disk selected by the machine profile instead of wherever
    /// Docker stores named volumes.
    pub(super) fn mounts(host: &LinuxHost, runner: &LinuxRunner) -> Vec<(String, &'static str)> {
        let mut mounts = vec![
            ("kithara-ci-cargo-home".to_owned(), "/home/runner/.cargo"),
            (
                Self::target_dir(host, runner)
                    .to_string_lossy()
                    .into_owned(),
                "/cache/target",
            ),
            ("kithara-ci-sccache".to_owned(), "/cache/sccache"),
            ("kithara-ci-fixtures".to_owned(), "/cache/fixtures"),
        ];
        if matches!(runner.flavor, RunnerFlavor::Plain) {
            mounts.push((
                Self::cargo_reapi_dir(host).to_string_lossy().into_owned(),
                "/cache/cargo-reapi",
            ));
        }
        mounts
    }

    pub(super) fn target_dir(host: &LinuxHost, runner: &LinuxRunner) -> PathBuf {
        host.cache_root.join("target").join(&runner.name)
    }

    pub(super) fn cargo_reapi_dir(host: &LinuxHost) -> PathBuf {
        host.cache_root.join("cargo-reapi")
    }

    pub(super) fn mount_type(source: &str) -> &'static str {
        if Path::new(source).is_absolute() {
            "bind"
        } else {
            "volume"
        }
    }

    /// What the job is told about where to build and what to reuse.
    ///
    /// Plain runners use cargo-reapi because the Linux test cost is dominated
    /// by links, which sccache does not store. Each runner still owns its Cargo
    /// target; verified compiler and linker outputs are shared by content.
    ///
    /// The linker entries come from [`LINUX_LINKER_ENV`], which the GitLab lane
    /// executor reads too: one statement of what a Linux job links with rather
    /// than one per way of starting a job.
    pub(super) fn environment(runner: &LinuxRunner) -> Vec<String> {
        let mut environment: Vec<String> = Self::CACHE_ENVIRONMENT
            .iter()
            .map(|entry| (*entry).to_owned())
            .collect();
        environment.extend(
            match runner.flavor {
                RunnerFlavor::Plain => Self::CARGO_REAPI_ENVIRONMENT.as_slice(),
                RunnerFlavor::Android => Self::SCCACHE_ENVIRONMENT.as_slice(),
            }
            .iter()
            .map(|entry| (*entry).to_owned()),
        );
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
        mounts: Container::mounts(host, runner),
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
        let host = crate::ci::linux::profile::tests::host_fixture();
        let runner = host.runner("kithara-ci-octocat").expect("runner");
        let environment = Container::environment(runner);

        for (name, value) in LINUX_LINKER_ENV {
            assert!(
                environment.contains(&format!("{name}={value}")),
                "{name} is missing from {environment:?}"
            );
        }
    }

    #[test]
    fn plain_runners_share_link_outputs_while_android_keeps_sccache() {
        let host = crate::ci::linux::profile::tests::host_fixture();
        let plain = host.runner("kithara-ci-octocat").expect("runner");
        let android = host.runner("kithara-ci-octocat-android").expect("runner");

        assert!(
            Container::environment(plain)
                .iter()
                .any(|entry| entry == "RUSTC_WRAPPER=kithara-cargo-reapi")
        );
        assert!(
            Container::environment(android)
                .iter()
                .any(|entry| entry == "RUSTC_WRAPPER=sccache")
        );
        assert!(
            Container::mounts(&host, plain)
                .iter()
                .any(|(_, target)| *target == "/cache/cargo-reapi")
        );
        assert!(
            Container::mounts(&host, plain)
                .iter()
                .any(|(_, target)| *target == "/cache/sccache")
        );
    }
}
