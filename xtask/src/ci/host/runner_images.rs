use std::{fs, path::Path, process::Stdio, thread, time::Duration};

use anyhow::{Context, Result, bail};
use reqwest::{
    blocking::Client,
    header::{ACCEPT, AUTHORIZATION},
};
use serde::Deserialize;
use tracing::info;

use super::runners::{
    RunnerManager, docker_host, path_text, read_trimmed, require_macos, write_secure,
};

const CILICON_ACCEPT: &str = "application/vnd.oci.image.manifest.v1+json";

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
                "CMAKE_AMD64_SHA256",
                self.config.pins.cmake_linux_amd64_sha256.as_str(),
            ),
            (
                "CMAKE_ARM64_SHA256",
                self.config.pins.cmake_linux_arm64_sha256.as_str(),
            ),
            (
                "GECKODRIVER_VERSION",
                self.config.pins.geckodriver_version.as_str(),
            ),
            (
                "GECKODRIVER_AMD64_SHA256",
                self.config.pins.geckodriver_linux_amd64_sha256.as_str(),
            ),
            (
                "GECKODRIVER_ARM64_SHA256",
                self.config.pins.geckodriver_linux_arm64_sha256.as_str(),
            ),
            (
                "GITLEAKS_VERSION",
                self.config.pins.gitleaks_version.as_str(),
            ),
            (
                "GITLEAKS_AMD64_SHA256",
                self.config.pins.gitleaks_linux_amd64_sha256.as_str(),
            ),
            (
                "GITLEAKS_ARM64_SHA256",
                self.config.pins.gitleaks_linux_arm64_sha256.as_str(),
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
            ("CARGO_SEMVER_CHECKS_VERSION", "cargo-semver-checks"),
            ("CARGO_SHEAR_VERSION", "cargo-shear"),
            ("CARGO_SORT_VERSION", "cargo-sort"),
            ("JUST_VERSION", "just"),
            ("MD_FORMATTER_VERSION", "md-formatter"),
            ("SCCACHE_VERSION", "sccache"),
            ("SIMILARITY_RS_VERSION", "similarity-rs"),
            ("TAPLO_CLI_VERSION", "taplo-cli"),
            ("TIDY_JSON_VERSION", "tidy-json"),
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

    pub(super) fn start_cilicon(&self) -> Result<()> {
        require_macos()?;
        self.require_ci_user()?;
        let expected = read_trimmed(
            &self
                .ci_home()
                .join(".config/kithara-ci/cilicon-image.digest"),
        )?;
        let actual = self.remote_cilicon_digest()?;
        if expected != self.config.pins.cilicon_image_digest || actual != expected {
            bail!(
                "Cilicon image digest changed: configured {}, pinned {expected}, remote {actual}",
                self.config.pins.cilicon_image_digest
            );
        }
        self.process.run(
            path_text(
                &self
                    .ci_home()
                    .join("Applications/Cilicon.app/Contents/MacOS/Cilicon"),
            )?,
            &[],
            "run Cilicon",
        )
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

    fn remote_cilicon_digest(&self) -> Result<String> {
        let image = self
            .config
            .pins
            .cilicon_image
            .strip_prefix("oci://ghcr.io/")
            .context("cilicon_image must use oci://ghcr.io/")?;
        let (repository, tag) = image
            .rsplit_once(':')
            .context("cilicon_image must include a tag")?;
        let client = Client::builder()
            .https_only(true)
            .build()
            .context("building GHCR client")?;
        let mut token_url =
            reqwest::Url::parse("https://ghcr.io/token").context("parsing GHCR token URL")?;
        token_url
            .query_pairs_mut()
            .append_pair("scope", &format!("repository:{repository}:pull"));
        let token: RegistryToken = client
            .get(token_url)
            .send()
            .context("requesting GHCR pull token")?
            .error_for_status()
            .context("GHCR pull token request failed")?
            .json()
            .context("decoding GHCR pull token")?;
        let response = client
            .head(format!("https://ghcr.io/v2/{repository}/manifests/{tag}"))
            .header(AUTHORIZATION, format!("Bearer {}", token.token))
            .header(ACCEPT, CILICON_ACCEPT)
            .send()
            .context("requesting Cilicon image manifest")?
            .error_for_status()
            .context("Cilicon image manifest request failed")?;
        response
            .headers()
            .get("docker-content-digest")
            .context("GHCR response has no Docker-Content-Digest")?
            .to_str()
            .context("GHCR digest header is not UTF-8")
            .map(ToOwned::to_owned)
    }
}

#[derive(Deserialize)]
struct RegistryToken {
    token: String,
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
