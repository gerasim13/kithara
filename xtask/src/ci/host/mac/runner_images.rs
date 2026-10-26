use std::{
    fs::{self, OpenOptions},
    path::Path,
    process::Stdio,
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use kithara_devtools::lock::{FileLock, Wait};
use sha2::{Digest, Sha256};
use tracing::info;

use super::{
    runner_guest::guest_developer_dir,
    runners::{RunnerManager, Tokens, docker_host, path_text, require_macos},
};
use crate::{ci::image::linux_build_args, consts};

impl RunnerManager<'_> {
    /// Prepare this commit's pinned image and return its immutable local reference.
    pub(super) fn build_linux_image(&self, dockerfile: &Path) -> Result<String> {
        require_macos()?;
        self.require_ci_user()?;
        permitted_pin(&self.config.pins.linux_image)?;
        if !dockerfile.is_file() {
            bail!("missing Linux CI Dockerfile: {}", dockerfile.display());
        }
        let home = self.ci_home();
        let locks = self.config.host.host_root.join("workspaces/image-locks");
        fs::create_dir_all(&locks)?;
        let key = hex::encode(Sha256::digest(self.config.pins.linux_image.as_bytes()));
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(locks.join(format!("image-{key}.lock")))?;
        let holder = crate::job::lock_holder();
        let _guard = FileLock::exclusive(
            lock,
            &Wait {
                subject: "pinned Linux CI image",
                holder: &holder,
            },
        )?;
        let mut lookup = self.process.command(self.config.host.brew_tool("docker"));
        lookup
            .env(
                "DOCKER_HOST",
                docker_host(&home, &self.config.host.colima_profile),
            )
            .args(["image", "ls", "--no-trunc", "--quiet", "--filter"])
            .arg(format!("reference={}", self.config.pins.linux_image));
        let existing = self
            .process
            .capture_command(&mut lookup, "find pinned Linux image")?;
        if existing.is_empty() {
            self.build_pinned_linux_image(&home, dockerfile)?;
        }
        let digest = self.linux_image_digest(&home, &self.config.pins.linux_image)?;
        if !valid_digest(&digest) {
            bail!("Docker returned invalid Linux image digest: {digest}");
        }
        let mut command = self.process.command(self.config.host.brew_tool("docker"));
        command
            .env(
                "DOCKER_HOST",
                docker_host(&home, &self.config.host.colima_profile),
            )
            .args([
                "image",
                "inspect",
                "--format",
                "{{json .RepoDigests}}",
                &self.config.pins.linux_image,
            ]);
        let response = self
            .process
            .capture_command(&mut command, "read pinned image references")?;
        let references: Vec<String> =
            serde_json::from_str(&response).context("read Docker repository digests")?;
        let reference = repository_reference(&self.config.pins.linux_image, &references)?;
        let actual = self.linux_image_digest(&home, reference)?;
        ensure!(
            actual == digest,
            "immutable Linux image reference disagrees with its pinned image"
        );
        info!(image = reference, %digest, "Linux CI image prepared");
        Ok(reference.to_owned())
    }

    fn build_pinned_linux_image(&self, home: &Path, dockerfile: &Path) -> Result<()> {
        let context = self
            .config
            .host
            .host_root
            .join(format!("workspaces/tmp/linux-image.{}", std::process::id()));
        if context.exists() {
            bail!(
                "temporary Linux image context already exists: {}",
                context.display()
            );
        }
        fs::create_dir(&context)
            .with_context(|| format!("creating Docker build context {}", context.display()))?;
        let result = (|| {
            let mut command = self.process.command(self.config.host.brew_tool("docker"));
            command
                .env(
                    "DOCKER_HOST",
                    docker_host(home, &self.config.host.colima_profile),
                )
                .args([
                    "buildx",
                    "build",
                    "--file",
                    path_text(dockerfile)?,
                    "--load",
                    "--platform",
                    "linux/arm64",
                    "--progress",
                    "plain",
                    "--provenance=false",
                    "--tag",
                    &self.config.pins.linux_image,
                ]);
            for (name, value) in linux_build_args(&self.config.pins)? {
                command.arg("--build-arg").arg(format!("{name}={value}"));
            }
            command.arg(path_text(&context)?);
            self.process
                .run_command(&mut command, "build pinned Linux CI image")
        })();
        let cleanup = fs::remove_dir_all(&context)
            .with_context(|| format!("removing Docker build context {}", context.display()));
        result.and(cleanup)
    }

    pub(super) fn smoke_linux(&self, image: &str) -> Result<()> {
        require_macos()?;
        self.require_ci_user()?;
        let home = self.ci_home();
        ensure!(
            valid_reference(image),
            "Linux smoke requires a permitted immutable image reference"
        );
        let checks = [
            vec!["rustc".to_owned(), "--version".to_owned()],
            vec![
                "rustc".to_owned(),
                format!("+{}", self.config.pins.msrv_toolchain),
                "--version".to_owned(),
            ],
            vec![
                "rustc".to_owned(),
                format!("+{}", self.config.pins.nightly_toolchain),
                "--version".to_owned(),
            ],
            vec![
                "cargo".to_owned(),
                "nextest".to_owned(),
                "--version".to_owned(),
            ],
            vec![
                "cargo".to_owned(),
                "deny".to_owned(),
                "--version".to_owned(),
            ],
            vec![
                "cargo".to_owned(),
                "machete".to_owned(),
                "--version".to_owned(),
            ],
            vec![
                "cargo".to_owned(),
                "mutants".to_owned(),
                "--version".to_owned(),
            ],
            vec!["wasm-pack".to_owned(), "--version".to_owned()],
            vec!["kache".to_owned(), "--version".to_owned()],
            vec!["sccache".to_owned(), "--version".to_owned()],
            vec!["ld.lld".to_owned(), "--version".to_owned()],
            vec!["mold".to_owned(), "--version".to_owned()],
            vec!["ninja".to_owned(), "--version".to_owned()],
            vec!["rc".to_owned(), "--version".to_owned()],
            vec!["zstd".to_owned(), "--version".to_owned()],
            vec!["firefox".to_owned(), "--version".to_owned()],
            vec!["chromium".to_owned(), "--version".to_owned()],
        ];
        for check in checks {
            let mut command = self.process.command(self.config.host.brew_tool("docker"));
            command
                .env(
                    "DOCKER_HOST",
                    docker_host(&home, &self.config.host.colima_profile),
                )
                .args(["run", "--rm", image])
                .args(&check);
            self.process
                .run_command(&mut command, "verify Linux CI image tool")?;
        }
        info!(image, "Linux image smoke passed");
        Ok(())
    }

    pub(super) fn smoke_android(&self) -> Result<()> {
        require_macos()?;
        self.require_ci_user()?;
        let emulator = self.config.host.android_home.join("emulator/emulator");
        let adb = self.config.host.android_home.join("platform-tools/adb");
        let log_path = self
            .config
            .host
            .host_root
            .join("logs/android-emulator-smoke.log");
        self.process.run(
            path_text(&emulator)?,
            &["-accel-check"],
            "verify Android acceleration",
        )?;
        let _ = self.process.command(&adb).args(["kill-server"]).status();
        let log = fs::File::create(&log_path)
            .with_context(|| format!("creating emulator log {}", log_path.display()))?;
        let stderr = log.try_clone().context("cloning emulator log handle")?;
        let mut child = self
            .process
            .command(&emulator)
            .env("ANDROID_HOME", &self.config.host.android_home)
            .env(
                "ANDROID_USER_HOME",
                self.config.host.host_root.join("toolchains/android-user"),
            )
            .env(
                "ANDROID_AVD_HOME",
                self.config
                    .host
                    .host_root
                    .join("toolchains/android-user/avd"),
            )
            .args([
                "-avd",
                &self.config.pins.android_avd,
                "-gpu",
                "swiftshader_indirect",
                "-no-audio",
                "-no-boot-anim",
                "-no-snapshot",
                "-no-window",
            ])
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(stderr))
            .spawn()
            .context("starting Android emulator")?;
        let result = (|| {
            self.process.run(
                path_text(&adb)?,
                &["wait-for-device"],
                "wait for Android emulator",
            )?;
            for _ in 0..120 {
                let boot = self
                    .process
                    .capture(
                        path_text(&adb)?,
                        &["shell", "getprop", "sys.boot_completed"],
                        "Android boot status",
                    )
                    .unwrap_or_default();
                if boot.trim() == "1" {
                    info!("Android emulator smoke passed");
                    return Ok(());
                }
                thread::sleep(Duration::from_secs(2));
            }
            bail!(
                "Android emulator did not finish booting; inspect {}",
                log_path.display()
            )
        })();
        let _ = self.process.command(&adb).args(["emu", "kill"]).status();
        let _ = child.kill();
        let _ = child.wait();
        result
    }

    /// Serves `GitLab` jobs from throwaway macOS VMs: each job gets a fresh clone
    /// of the pinned base image, and the clone is destroyed afterwards. Xcode
    /// and the Rust toolchain are mounted from the host, so the image itself
    /// stays a plain macOS install.
    pub(super) fn run_macos_runner(&self) -> Result<()> {
        require_macos()?;
        self.require_ci_user()?;
        self.verify_macos_base()?;
        let tart = path_text(&self.config.host.brew_tool("tart"))?.to_string();
        // One guest serves every job it is offered. Recreating it per job left
        // the queue waiting for a runner that did not exist yet, and the guest
        // is thrown away and rebuilt from the base image whenever it does stop,
        // so a job never inherits a half-finished predecessor.
        loop {
            self.destroy_job_vm(&tart);
            self.process.run(
                &tart,
                &["clone", self.base_vm_name()?, consts::JOB_VM_NAME],
                "clone CI macOS VM",
            )?;
            let outcome = self
                .boot_job_vm(&tart)
                .and_then(|address| self.serve_jobs(&address));
            self.destroy_job_vm(&tart);
            outcome?;
        }
    }

    fn verify_macos_base(&self) -> Result<()> {
        let bundle = &self.config.host.macos_vm_bundle;
        for part in ["config.json", "disk.img", "nvram.bin"] {
            let path = bundle.join(part);
            if !path.is_file() {
                bail!(
                    "CI macOS VM bundle is incomplete: {} is missing. Build it with \
                     `tart create --from-ipsw` for macOS build {}",
                    path.display(),
                    self.config.pins.macos_guest_build
                );
            }
        }
        Ok(())
    }

    fn base_vm_name(&self) -> Result<&str> {
        self.config
            .host
            .macos_vm_bundle
            .file_name()
            .and_then(|name| name.to_str())
            .context("macos_vm_bundle has no VM name")
    }

    fn boot_job_vm(&self, tart: &str) -> Result<String> {
        let mut command = self.process.command(tart);
        command
            // `--no-audio` is what makes the iOS Simulator usable here. With
            // the pass-through on, the guest's virtual sound device carries an
            // input channel that leads nowhere, and `AudioUnitInitialize` on
            // `AURemoteIO` inside a simulator blocks on it: CoreAudio waits
            // nine seconds, reports "Initialize: RPC timeout. Apparently
            // deadlocked" and aborts the process. Every test that starts
            // playback died that way. Without the pass-through the device is
            // output-only and the same unit initializes in two seconds. The
            // jobs never play audible sound, so nothing is lost.
            .args(["run", "--no-graphics", "--no-audio"])
            .args(self.job_vm_mounts())
            .arg(consts::JOB_VM_NAME)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command.spawn().context("starting CI macOS VM")?;
        for _ in 0..consts::BOOT_ATTEMPTS {
            thread::sleep(consts::BOOT_POLL);
            if let Ok(address) =
                self.process
                    .capture(tart, &["ip", consts::JOB_VM_NAME], "CI macOS VM address")
                && !address.trim().is_empty()
            {
                return Ok(address.trim().to_string());
            }
        }
        bail!("CI macOS VM did not report an address within the boot timeout")
    }

    fn job_vm_mounts(&self) -> Vec<String> {
        let root = &self.config.host.host_root;
        let home = self.ci_home();
        let xcode = self
            .config
            .host
            .host_xcode_app()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        [
            ("Xcode.app", xcode, true),
            // `kithara-encode` links ffmpeg, and several `*-sys` crates shell
            // out to cmake and pkg-config. A stock macOS image carries none of
            // them, so share the host prefix; the guest links it back to the
            // canonical path, which the install names in these dylibs need.
            ("kithara-brew", self.config.host.brew_root.clone(), true),
            ("kithara-tools", root.join("toolchains/shared-bin"), true),
            // virtiofs cannot serve rustup's downloads, so the guest must find
            // every toolchain already installed here.
            ("kithara-rustup", home.join(".rustup"), true),
            // Cargo writes its registry and git checkouts; `concurrent = 1`
            // keeps one job from racing another over the same cache.
            ("kithara-cargo", home.join(".cargo"), false),
            ("kithara-cache", root.join("cache"), false),
        ]
        .into_iter()
        .map(|(tag, path, read_only)| {
            let suffix = if read_only { ":ro" } else { "" };
            format!("--dir={tag}:{}{suffix}", path.display())
        })
        .collect()
    }

    fn serve_jobs(&self, address: &str) -> Result<()> {
        let shared = consts::GUEST_SHARE;
        let brew = self.config.host.brew_root.join("bin");
        let brew = brew.display();
        let path = format!("$HOME/.cargo/bin:{brew}:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin");
        let tokens = Tokens::load(
            &self.ci_home().join(".config/kithara-ci"),
            self.config.host.job_concurrency,
        )?;
        // Move the share off the auto-mounted path before anything reads it:
        // its name contains spaces, which GNU make cannot express. Nothing may
        // be running from the mount at this point, so this is the only moment
        // the unmount can succeed.
        self.guest_shell(
            address,
            &format!(
                "set -e; \
                 if [ ! -d {shared}/Xcode.app ]; then \
                 sudo -n diskutil unmount force '{}' >/dev/null; \
                 sudo -n install -d -m 0755 {shared}; \
                 sudo -n mount_virtiofs com.apple.virtio-fs.automount {shared}; \
                 fi",
                self.config.host.macos_guest_shared_root.display(),
            ),
            "remount guest share without spaces",
            None,
        )?;
        self.guest_shell(
            address,
            &format!(
                "export PATH={path}; \
                 {shared}/kithara-tools/kithara-ci ci host mac --config \
                 {shared}/kithara-tools/mac-host.toml --pins \
                 {shared}/kithara-tools/pins.toml guest-prepare"
            ),
            "prepare CI macOS guest",
            None,
        )?;
        // One guest serves a run of jobs rather than one, so the second job of
        // a pipeline does not wait for a new one to boot — but it serves a
        // bounded run, because the space each job leaves behind comes back
        // only when the guest is destroyed.
        self.guest_shell(
            address,
            &format!(
                "read -r RUNNER_TOKEN; \
                 export PATH={path}; \
                 export DEVELOPER_DIR={}; \
                 export KITHARA_CI_CACHE_ROOT={shared}/kithara-cache; \
                 exec {shared}/kithara-tools/gitlab-runner run-single --url {} \
                 --token \"$RUNNER_TOKEN\" --executor shell --shell bash \
                 --max-builds {} --wait-timeout {}",
                guest_developer_dir(),
                self.config.host.gitlab_origin(),
                consts::MAX_BUILDS,
                consts::WAIT_SECONDS,
            ),
            "serve GitLab jobs",
            tokens.macos.first().map(String::as_str),
        )
    }

    /// The runner token reaches the guest over stdin so it never lands in
    /// `argv`, where any local process could read it.
    fn guest_shell(
        &self,
        address: &str,
        script: &str,
        label: &str,
        stdin: Option<&str>,
    ) -> Result<()> {
        let mut command = self.process.command("/usr/bin/ssh");
        command
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
                &format!("{}@{address}", self.config.host.macos_guest_user),
                script,
            ])
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        let mut child = command
            .spawn()
            .with_context(|| format!("running guest command: {label}"))?;
        if let Some(secret) = stdin
            && let Some(pipe) = child.stdin.as_mut()
        {
            use std::io::Write;
            writeln!(pipe, "{secret}").context("passing the runner token to the guest")?;
        }
        let status = child
            .wait()
            .with_context(|| format!("waiting for guest command: {label}"))?;
        if !status.success() {
            bail!(
                "{label} failed with exit code {}",
                status.code().unwrap_or(-1)
            );
        }
        Ok(())
    }

    fn destroy_job_vm(&self, tart: &str) {
        self.process
            .best_effort(tart, &["stop", consts::JOB_VM_NAME], "stop CI macOS VM");
        self.process
            .best_effort(tart, &["delete", consts::JOB_VM_NAME], "delete CI macOS VM");
    }

    fn linux_image_digest(&self, home: &Path, image: &str) -> Result<String> {
        let mut command = self.process.command(self.config.host.brew_tool("docker"));
        command
            .env(
                "DOCKER_HOST",
                docker_host(home, &self.config.host.colima_profile),
            )
            .args(["image", "inspect", "--format", "{{.Id}}", image]);
        self.process
            .capture_command(&mut command, "inspect Linux image identity")
    }
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn valid_reference(value: &str) -> bool {
    value
        .strip_prefix(consts::LINUX_IMAGE_REPOSITORY)
        .and_then(|suffix| suffix.strip_prefix('@'))
        .is_some_and(valid_digest)
}

fn repository_reference<'a>(pinned: &str, references: &'a [String]) -> Result<&'a str> {
    permitted_pin(pinned)?;
    references
        .iter()
        .find(|reference| valid_reference(reference))
        .map(String::as_str)
        .context("pinned Linux image has no local immutable repository digest")
}

fn permitted_pin(pinned: &str) -> Result<()> {
    ensure!(
        pinned.split_once(':').map(|(repo, _)| repo) == Some(consts::LINUX_IMAGE_REPOSITORY),
        "Linux image pin uses an unpermitted repository"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use crate::ci::{
        config::fixture,
        process::{Process, Recording},
    };

    #[test]
    fn only_a_real_digest_of_the_permitted_repository_is_selected() {
        let digest = format!("kithara-ci@sha256:{}", "a".repeat(64));
        let references = vec![format!("other@sha256:{}", "a".repeat(64)), digest.clone()];
        assert_eq!(
            repository_reference("kithara-ci:linux-test", &references).unwrap(),
            digest
        );
        assert!(repository_reference("other:linux-test", &references).is_err());
        assert!(
            repository_reference("kithara-ci:linux-test", &["kithara-ci:linux-latest".into()])
                .is_err()
        );
        assert!(repository_reference("kithara-ci:linux-test", &[]).is_err());
        assert!(!valid_reference("kithara-ci-foreign@sha256:abcd"));
    }

    #[test]
    fn image_digest_is_strict() {
        assert!(valid_digest(&format!("sha256:{}", "a".repeat(64))));
        assert!(!valid_digest(&format!("sha256:{}", "a".repeat(63))));
        assert!(!valid_digest(&format!("md5:{}", "a".repeat(64))));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn image_preparation_uses_owned_paths_and_smokes_its_immutable_identity() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let mut config = fixture();
        config.host.host_root = directory.path().to_path_buf();
        let dockerfile = directory.path().join("Dockerfile");
        fs::write(&dockerfile, "FROM scratch\n").unwrap();
        let id = format!("sha256:{}", "a".repeat(64));
        let reference = format!("kithara-ci@sha256:{}", "b".repeat(64));
        let references = serde_json::to_string(&vec![reference.clone()]).unwrap();
        let recording = Recording::default()
            .with_reply(&["/usr/bin/id", "-un"], &config.host.ci_user)
            .with_reply(&["/opt/homebrew/bin/docker", "image", "ls"], &id)
            .with_reply(
                &[
                    "/opt/homebrew/bin/docker",
                    "image",
                    "inspect",
                    "--format",
                    "{{.Id}}",
                ],
                &id,
            )
            .with_reply(
                &[
                    "/opt/homebrew/bin/docker",
                    "image",
                    "inspect",
                    "--format",
                    "{{json .RepoDigests}}",
                ],
                &references,
            );
        let process = Process::recording(directory.path(), recording);
        let manager = RunnerManager::new(&config, &process);
        fs::create_dir(directory.path().join("workspaces")).unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o500)).unwrap();
        let prepared = manager.build_linux_image(&dockerfile);
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let prepared = prepared.unwrap();
        assert_eq!(prepared, reference);
        assert!(!directory.path().join("locks").exists());
        assert_eq!(
            fs::read_dir(directory.path().join("workspaces/image-locks"))
                .unwrap()
                .count(),
            1
        );
        manager.smoke_linux(&prepared).unwrap();
        let steps = process.recorded().unwrap();
        let runs: Vec<_> = steps
            .steps()
            .iter()
            .filter(|step| step.args.first().is_some_and(|arg| arg == "run"))
            .collect();
        assert!(!runs.is_empty());
        assert!(runs.iter().all(|step| step.args[2] == reference));
        assert!(
            !steps
                .steps()
                .iter()
                .any(|step| step.args.first().is_some_and(|arg| arg == "tag"))
        );
        assert!(
            !directory
                .path()
                .join("home/kithara-ci/.config/kithara-ci/linux-image.digest")
                .exists()
        );
    }
}
