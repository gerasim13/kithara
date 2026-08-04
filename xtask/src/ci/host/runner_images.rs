use std::{fs, path::Path, process::Stdio, thread, time::Duration};

use anyhow::{Context, Result, bail};
use tracing::info;

use super::{
    runner_guest::{GUEST_SHARE, guest_developer_dir},
    runners::{
        RunnerManager, Tokens, docker_host, path_text, read_trimmed, require_macos, write_secure,
    },
};

/// The throwaway VM the macOS lane clones for every job.
struct JobVm;

impl JobVm {
    const NAME: &'static str = "kithara-ci-job";
    const BOOT_ATTEMPTS: u32 = 40;
    const BOOT_POLL: Duration = Duration::from_secs(5);
    const WAIT_SECONDS: u32 = 7200;
}

impl RunnerManager<'_> {
    pub(super) fn build_linux_image(&self, dockerfile: &Path) -> Result<()> {
        require_macos()?;
        self.require_ci_user()?;
        if !dockerfile.is_file() {
            bail!("missing Linux CI Dockerfile: {}", dockerfile.display());
        }
        let home = self.ci_home();
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
            command.env("DOCKER_HOST", docker_host(&home)).args([
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
            for (name, value) in self.linux_build_args()? {
                command.arg("--build-arg").arg(format!("{name}={value}"));
            }
            command.arg(path_text(&context)?);
            self.process
                .run_command(&mut command, "build pinned Linux CI image")?;
            let digest = self.linux_image_digest(&home)?;
            if !valid_digest(&digest) {
                bail!("Docker returned invalid Linux image digest: {digest}");
            }
            let config_root = home.join(".config/kithara-ci");
            fs::create_dir_all(&config_root)?;
            write_secure(
                &config_root.join("linux-image.digest"),
                &format!("{digest}\n"),
            )?;
            info!(image = self.config.pins.linux_image, %digest, "Linux CI image built");
            Ok(())
        })();
        let cleanup = fs::remove_dir_all(&context)
            .with_context(|| format!("removing Docker build context {}", context.display()));
        result.and(cleanup)
    }

    fn linux_build_args(&self) -> Result<Vec<(&'static str, &str)>> {
        let mut args = vec![
            ("RUST_VERSION", self.config.pins.stable_toolchain.as_str()),
            (
                "RUST_BASE_DIGEST",
                self.config.pins.linux_base_digest.as_str(),
            ),
            ("MSRV_TOOLCHAIN", self.config.pins.msrv_toolchain.as_str()),
            (
                "NIGHTLY_TOOLCHAIN",
                self.config.pins.nightly_toolchain.as_str(),
            ),
            ("CMAKE_VERSION", self.config.pins.cmake_version.as_str()),
            (
                "CMAKE_SHA256",
                self.config.pins.cmake_linux_arm64_sha256.as_str(),
            ),
            (
                "GECKODRIVER_VERSION",
                self.config.pins.geckodriver_version.as_str(),
            ),
            (
                "GECKODRIVER_SHA256",
                self.config.pins.geckodriver_linux_arm64_sha256.as_str(),
            ),
            (
                "GITLEAKS_VERSION",
                self.config.pins.gitleaks_version.as_str(),
            ),
            (
                "GITLEAKS_SHA256",
                self.config.pins.gitleaks_linux_arm64_sha256.as_str(),
            ),
            (
                "SANDBOX_RUNTIME_VERSION",
                self.config.pins.sandbox_runtime_version.as_str(),
            ),
        ];
        for (name, tool) in [
            ("AST_GREP_VERSION", "ast-grep"),
            ("CARGO_DENY_VERSION", "cargo-deny"),
            ("CARGO_HACK_VERSION", "cargo-hack"),
            ("CARGO_LLVM_COV_VERSION", "cargo-llvm-cov"),
            ("CARGO_MACHETE_VERSION", "cargo-machete"),
            ("CARGO_MUTANTS_VERSION", "cargo-mutants"),
            ("CARGO_NEXTEST_VERSION", "cargo-nextest"),
            ("CARGO_REAPI_VERSION", "cargo-reapi"),
            ("CARGO_SEMVER_CHECKS_VERSION", "cargo-semver-checks"),
            ("CARGO_SHEAR_VERSION", "cargo-shear"),
            ("CARGO_SORT_VERSION", "cargo-sort"),
            ("JUST_VERSION", "just"),
            ("MD_FORMATTER_VERSION", "md-formatter"),
            ("SCCACHE_VERSION", "sccache"),
            ("SIMILARITY_RS_VERSION", "similarity-rs"),
            ("TAPLO_CLI_VERSION", "taplo-cli"),
            ("TIDY_JSON_VERSION", "tidy-json"),
            ("TRUNK_VERSION", "trunk"),
            ("TYPOS_CLI_VERSION", "typos-cli"),
            ("WASM_BINDGEN_CLI_VERSION", "wasm-bindgen-cli"),
            ("WASM_PACK_VERSION", "wasm-pack"),
            ("WASM_SLIM_VERSION", "wasm-slim"),
        ] {
            args.push((name, self.config.pins.cargo_tool_version(tool)?));
        }
        Ok(args)
    }

    pub(super) fn smoke_linux(&self) -> Result<()> {
        require_macos()?;
        self.require_ci_user()?;
        let home = self.ci_home();
        let expected = read_trimmed(&home.join(".config/kithara-ci/linux-image.digest"))?;
        let actual = self.linux_image_digest(&home)?;
        if actual != expected {
            bail!("Linux CI image digest changed: expected {expected}, found {actual}");
        }
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
            vec!["sccache".to_owned(), "--version".to_owned()],
            vec!["firefox".to_owned(), "--version".to_owned()],
            vec!["chromium".to_owned(), "--version".to_owned()],
        ];
        for check in checks {
            let mut command = self.process.command(self.config.host.brew_tool("docker"));
            command
                .env("DOCKER_HOST", docker_host(&home))
                .args(["run", "--rm", &self.config.pins.linux_image])
                .args(&check);
            self.process
                .run_command(&mut command, "verify Linux CI image tool")?;
        }
        info!(
            image = self.config.pins.linux_image,
            digest = actual,
            "Linux image smoke passed"
        );
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
                &["clone", self.base_vm_name()?, JobVm::NAME],
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
            .args(["run", "--no-graphics"])
            .args(self.job_vm_mounts())
            .arg(JobVm::NAME)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command.spawn().context("starting CI macOS VM")?;
        for _ in 0..JobVm::BOOT_ATTEMPTS {
            thread::sleep(JobVm::BOOT_POLL);
            if let Ok(address) =
                self.process
                    .capture(tart, &["ip", JobVm::NAME], "CI macOS VM address")
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
        let shared = GUEST_SHARE;
        let brew = self.config.host.brew_root.join("bin");
        let brew = brew.display();
        let path = format!("$HOME/.cargo/bin:{brew}:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin");
        let tokens = Tokens::load(&self.ci_home().join(".config/kithara-ci"))?;
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
                 {shared}/kithara-tools/kithara-ci ci host --config \
                 {shared}/kithara-tools/host.toml --pins \
                 {shared}/kithara-tools/pins.toml guest-prepare"
            ),
            "prepare CI macOS guest",
            None,
        )?;
        // No `--max-builds`: the guest keeps taking jobs until it idles out,
        // so the second job of a pipeline does not wait for a new one to boot.
        self.guest_shell(
            address,
            &format!(
                "read -r RUNNER_TOKEN; \
                 export PATH={path}; \
                 export DEVELOPER_DIR={}; \
                 export KITHARA_CI_CACHE_ROOT={shared}/kithara-cache; \
                 exec {shared}/kithara-tools/gitlab-runner run-single --url {} \
                 --token \"$RUNNER_TOKEN\" --executor shell --shell bash \
                 --wait-timeout {}",
                guest_developer_dir(),
                self.config.host.gitlab_origin(),
                JobVm::WAIT_SECONDS,
            ),
            "serve GitLab jobs",
            Some(&tokens.macos),
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
            .best_effort(tart, &["stop", JobVm::NAME], "stop CI macOS VM");
        self.process
            .best_effort(tart, &["delete", JobVm::NAME], "delete CI macOS VM");
    }

    pub(super) fn linux_image_digest(&self, home: &Path) -> Result<String> {
        let mut command = self.process.command(self.config.host.brew_tool("docker"));
        command.env("DOCKER_HOST", docker_host(home)).args([
            "image",
            "inspect",
            "--format",
            "{{.Id}}",
            &self.config.pins.linux_image,
        ]);
        let output = command
            .output()
            .context("starting Docker image inspection")?;
        if !output.status.success() {
            bail!(
                "Docker image inspection failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        String::from_utf8(output.stdout)
            .context("Docker image digest is not UTF-8")
            .map(|digest| digest.trim().to_owned())
    }
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::ci::{config::fixture, process::Process};

    #[test]
    fn image_digest_is_strict() {
        assert!(valid_digest(&format!("sha256:{}", "a".repeat(64))));
        assert!(!valid_digest(&format!("sha256:{}", "a".repeat(63))));
        assert!(!valid_digest(&format!("md5:{}", "a".repeat(64))));
    }

    #[test]
    fn dockerfile_versions_are_owned_by_typed_config() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let config = fixture();
        let process = Process::new(&root, BTreeMap::new());
        let manager = RunnerManager::new(&config, &process);
        let configured = manager
            .linux_build_args()
            .unwrap()
            .into_iter()
            .map(|(name, _)| name)
            .collect::<BTreeSet<_>>();
        let dockerfile = fs::read_to_string(root.join("docker/ci.Dockerfile")).unwrap();
        let declared = dockerfile
            .lines()
            .filter_map(|line| line.strip_prefix("ARG "))
            .inspect(|argument| {
                assert!(
                    !argument.contains('='),
                    "Docker build argument must not own a default: {argument}"
                );
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(declared, configured);
    }
}
