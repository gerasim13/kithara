use std::path::{Path, PathBuf};

use anyhow::Result;

use super::profile::{LinuxHost, LinuxRunner, RunnerFlavor};
use crate::{
    ci::{config::CiPins, image::floating_tag},
    consts,
};

/// What one runner's container is, independent of who starts it.
///
/// systemd starts these through a `docker run` line and Compose through a
/// service block. Both are renderings of this: a second description would drift
/// from the first, and the drift would be a runner that quietly ran with the
/// wrong cores, the wrong image, or no device.
pub(super) struct Container<'a> {
    pub(super) name: String,
    /// The floating tag, not the pin: the pin says what to build, and a
    /// container that named it would die the moment the pin moved ahead of
    /// what this machine has built.
    pub(super) image: String,
    pub(super) network: &'a str,
    /// Which cores its jobs may use, as a set rather than a share. See
    /// [`super::services::cpuset`].
    pub(super) cpuset: String,
    pub(super) memory: &'a str,
    /// The slice that caps the fleet as a whole. A runner's own `memory` never
    /// fired: the host livelocked on the sum of ceilings several times its
    /// memory, so every container draws on one budget instead.
    pub(super) cgroup_parent: &'static str,
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
    /// The cache mounts every runner shares, and the build directory it keeps
    /// to itself under the host's configured cache root.
    ///
    /// The registry of downloaded crates is shared because that is what it is
    /// for, and the compiler cache because `sccache` keys on the inputs of a
    /// compilation, so one runner's entry is another's hit.
    ///
    /// The build root is shared, and a lane claims the directory named after it
    /// underneath. Build artefacts are valid only for the exact features,
    /// profile and toolchain that produced them, which is why one directory for
    /// every job reuses nothing — but a lane asks for the same shape on every
    /// run, so the directory it claims is warm whichever runner picked the job
    /// up. A runner-owned directory instead decided reuse by which runner
    /// happened to be free, and a lane that moved compiled the workspace again.
    /// A host path keeps that write-heavy cache on the disk selected by the
    /// machine profile instead of wherever Docker stores named volumes.
    pub(super) fn mounts(host: &LinuxHost, runner: &LinuxRunner) -> Vec<(String, &'static str)> {
        vec![
            ("kithara-ci-cargo-home".to_owned(), "/home/runner/.cargo"),
            (
                host.cache_root
                    .join("workspaces")
                    .join(&runner.name)
                    .to_string_lossy()
                    .into_owned(),
                "/runner/_work",
            ),
            (
                Self::target_dir(host, runner)
                    .to_string_lossy()
                    .into_owned(),
                "/cache/target",
            ),
            (
                Self::lane_root(host).to_string_lossy().into_owned(),
                "/cache/lanes",
            ),
            ("kithara-ci-sccache".to_owned(), "/cache/sccache"),
            ("kithara-ci-fixtures".to_owned(), "/cache/fixtures"),
        ]
    }

    /// Where a job that claims no lane directory builds. One per runner, which
    /// is what such a job reused before the lane keying existed.
    pub(super) fn target_dir(host: &LinuxHost, runner: &LinuxRunner) -> PathBuf {
        host.cache_root.join("target").join(&runner.name)
    }

    /// The one build root every runner mounts. A lane owns one directory under
    /// it; the budget is enforced over the root.
    pub(super) fn lane_root(host: &LinuxHost) -> PathBuf {
        host.cache_root.join("lanes")
    }

    /// Where every runner bootstraps xtask: one build per trust, under the CI
    /// cache root the workflows name (`KITHARA_CI_CACHE_ROOT`) beside the lane
    /// slots, so a commit's xtask is compiled once for the whole host.
    pub(super) fn bootstrap_root(host: &LinuxHost) -> PathBuf {
        Self::lane_root(host).join(".kithara-ci").join("bootstrap")
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
    /// The linker entries come from [`LINUX_LINKER_ENV`](consts::LINUX_LINKER_ENV), which the GitLab lane
    /// executor reads too: one statement of what a Linux job links with rather
    /// than one per way of starting a job.
    pub(super) fn environment(runner: &LinuxRunner) -> Vec<String> {
        let mut environment: Vec<String> = consts::CACHE_ENVIRONMENT
            .iter()
            .map(|entry| (*entry).to_owned())
            .collect();
        environment.push(format!(
            "SCCACHE_IDLE_TIMEOUT={SCCACHE_IDLE_TIMEOUT}",
            SCCACHE_IDLE_TIMEOUT = consts::SCCACHE_IDLE_TIMEOUT
        ));
        // The S3 backend is shared, but each runner needs its own daemon
        // endpoint. An explicit socket lets the lane start that daemon before
        // Cargo's parallel compilers can race to start it.
        environment.push(format!("SCCACHE_DIR=/cache/sccache/{}", runner.name));
        environment.push(format!("SCCACHE_SERVER_UDS=/tmp/{}.sock", runner.name));
        environment.extend(
            consts::LINUX_LINKER_ENV
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
) -> Result<Container<'a>> {
    Ok(Container {
        name: format!("kithara-ci-{}", runner.name),
        image: floating_tag(match runner.flavor {
            RunnerFlavor::Plain => &pins.linux_runner_image,
            RunnerFlavor::Android => &pins.linux_android_runner_image,
        })?,
        network: &host.network,
        cpuset,
        memory: &runner.memory,
        cgroup_parent: consts::SERVICE_SLICE,
        devices: &runner.devices,
        groups: &runner.groups,
        env_file: super::services::env_file(runner),
        mounts: Container::mounts(host, runner),
    })
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::{env, fs, os::unix::fs::PermissionsExt, process::Command};

    use super::*;

    /// The linker a Linux job links with is part of what a job is told, not a
    /// property of whichever image happened to be built: an unnamed linker is
    /// `bfd`, and `bfd` is where a test job spends more time than it spends
    /// testing.
    #[test]
    fn a_job_is_told_which_linker_to_use() {
        let host = super::super::profile::tests::host_fixture();
        let runner = host.runner("kithara-ci-octocat").expect("runner");
        let environment = Container::environment(runner);

        for (name, value) in consts::LINUX_LINKER_ENV {
            assert!(
                environment.contains(&format!("{name}={value}")),
                "{name} is missing from {environment:?}"
            );
        }
    }

    /// A lane's build directory moves between runners, and it records the
    /// downloaded beat models by path and modification time. Models kept where
    /// only one runner sees them are fetched again by the next, newer than the
    /// build, and everything that embeds them is rebuilt.
    #[test]
    fn a_job_keeps_the_beat_models_on_a_mount_every_runner_shares() {
        let host = super::super::profile::tests::host_fixture();
        let [first, second, ..] = host.runners.as_slice() else {
            panic!("the host fixture serves more than one runner");
        };
        let environment = Container::environment(first);
        let models = environment
            .iter()
            .find_map(|entry| entry.strip_prefix("KITHARA_BEAT_MODEL_CACHE="))
            .expect("a job is told where the beat models live");

        let shared = Container::mounts(&host, first)
            .into_iter()
            .zip(Container::mounts(&host, second))
            .filter(|(mine, theirs)| mine == theirs)
            .map(|((_, destination), _)| destination);
        assert!(
            shared
                .into_iter()
                .any(|destination| Path::new(models).starts_with(destination)),
            "{models} is not on a mount every runner shares"
        );
    }

    #[cfg(unix)]
    #[test]
    fn stable_runner_slots_own_bootstrap_targets_across_registrations() {
        let host = super::super::profile::tests::host_fixture();
        let first = host.runner("kithara-ci-octocat").expect("first runner");
        let second = host
            .runner("kithara-ci-octocat-gpu")
            .expect("second runner");
        let directory = tempfile::tempdir().expect("temp dir");
        let cache = directory.path().join("cache");
        let bin = directory.path().join("bin");
        let tool_log = directory.path().join("tools.log");
        fs::create_dir(&bin).expect("tool trap directory");
        for tool in ["cargo", "git"] {
            let path = bin.join(tool);
            fs::write(
                &path,
                b"#!/bin/sh\nprintf '%s\n' \"$0\" >> \"$BOOTSTRAP_TOOL_LOG\"\nexit 97\n",
            )
            .expect("tool trap");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .expect("executable tool trap");
        }
        let inherited_path = env::var_os("PATH").expect("PATH");
        let path = env::join_paths(
            std::iter::once(bin.clone()).chain(env::split_paths(&inherited_path)),
        )
        .expect("tool trap PATH");
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repository root");
        let target = |runner: &LinuxRunner, registration: &str| {
            let environment = Container::environment(runner);
            let output = Command::new("just")
                .current_dir(root)
                .arg("_xtask-self-target")
                .env_remove("CI_CONCURRENT_ID")
                .envs(environment.iter().map(|entry| {
                    entry.split_once('=').expect("container environment entry")
                }))
                .env("KITHARA_CI_CACHE_ROOT", &cache)
                .env("KITHARA_CACHE_TRUST", runner.cache_trust.as_str())
                .env("RUNNER_NAME", format!("{}-{registration}", runner.name))
                .env("CI_JOB_ID", registration)
                .env("CARGO", bin.join("cargo"))
                .env("PATH", &path)
                .env("BOOTSTRAP_TOOL_LOG", &tool_log)
                .output()
                .expect("real bootstrap target transport");
            assert!(
                output.status.success(),
                "bootstrap target failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            PathBuf::from(String::from_utf8(output.stdout).expect("target path").trim())
        };
        let first_target = target(first, "12345");
        let second_target = target(second, "12345");

        assert_eq!(first_target, target(first, "54321"));
        assert_eq!(second_target, target(second, "54321"));
        assert_ne!(first_target, second_target);
        let parent = cache.join("bootstrap/review");
        for (runner, target) in [(first, &first_target), (second, &second_target)] {
            assert_eq!(target.parent(), Some(parent.as_path()));
            assert!(
                target
                    .file_name()
                    .expect("target directory")
                    .to_string_lossy()
                    .ends_with(&format!("-{}", runner.name))
            );
        }
        assert!(!tool_log.exists(), "target selection invoked Cargo or Git");
    }

    #[test]
    fn a_runner_keeps_its_ready_cache_daemon_available_for_its_job() {
        let host = super::super::profile::tests::host_fixture();
        let runner = host.runner("kithara-ci-octocat").expect("runner");

        assert!(Container::environment(runner).contains(&format!(
            "SCCACHE_IDLE_TIMEOUT={SCCACHE_IDLE_TIMEOUT}",
            SCCACHE_IDLE_TIMEOUT = consts::SCCACHE_IDLE_TIMEOUT
        )));
    }
}
