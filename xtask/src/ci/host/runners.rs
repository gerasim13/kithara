use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use tracing::info;

use super::services::launchd;
use crate::ci::{
    config::{CiConfig, LANE_CONFIG_PATH},
    process::Process,
};

pub(super) struct RunnerManager<'a> {
    pub(super) config: &'a CiConfig,
    pub(super) process: &'a Process,
}

impl<'a> RunnerManager<'a> {
    pub(super) fn new(config: &'a CiConfig, process: &'a Process) -> Self {
        Self { config, process }
    }

    pub(super) fn configure(&self) -> Result<()> {
        require_macos()?;
        self.require_ci_user()?;
        let home = self.ci_home();
        let config_root = home.join(".config/kithara-ci");
        let runner_root = home.join(".gitlab-runner");
        let image_digest_path = config_root.join("linux-image.digest");
        let expected_linux = read_trimmed(&image_digest_path)?;
        let actual_linux = self.linux_image_digest(&home)?;
        if expected_linux != actual_linux {
            bail!("Linux CI image digest changed: expected {expected_linux}, found {actual_linux}");
        }
        let tokens = Tokens::load(&config_root)?;
        for path in [
            &config_root,
            &runner_root,
            &self.config.host.host_root.join("cache/gitlab-runner"),
            &self.config.host.host_root.join("toolchains/shared-bin"),
            &self.config.host.host_root.join("workspaces/gitlab"),
            &self.agent_root(),
        ] {
            fs::create_dir_all(path)
                .with_context(|| format!("creating runner directory {}", path.display()))?;
        }
        self.copy_shared_tool("xcodegen")?;
        let current = std::env::current_exe().context("resolving CI executable")?;
        replace_file(
            &current,
            &self
                .config
                .host
                .host_root
                .join("toolchains/shared-bin/kithara-ci"),
        )
        .context("installing CI executable for macOS guests")?;
        for name in ["host.toml", "pins.toml"] {
            replace_file(
                &self.config.host.host_root.join("services").join(name),
                &self
                    .config
                    .host
                    .host_root
                    .join("toolchains/shared-bin")
                    .join(name),
            )
            .with_context(|| format!("installing macOS guest {name}"))?;
        }

        write_secure(
            &runner_root.join("config.toml"),
            &self.runner_config(&home, &tokens),
        )?;
        self.install_runner_agents(&home)?;
        self.process.run(
            path_text(&self.config.host.brew_tool("gitlab-runner"))?,
            &[
                "verify",
                "--config",
                path_text(&runner_root.join("config.toml"))?,
            ],
            "verify GitLab runners",
        )?;
        info!("GitLab runner configuration installed");
        Ok(())
    }

    pub(super) fn activate(&self) -> Result<()> {
        require_macos()?;
        self.require_ci_user()?;
        let uid = self.process.capture("/usr/bin/id", &["-u"], "CI user id")?;
        let domain = format!("gui/{uid}");
        let status = self
            .process
            .command("/bin/launchctl")
            .args(["print", &domain])
            .status()
            .context("checking CI GUI session")?;
        if !status.success() {
            bail!(
                "the {} GUI session is not active; log in locally first",
                self.config.host.ci_user
            );
        }
        for name in [
            "cleanup",
            "health",
            "colima",
            "gitlab-runner",
            "macos-runner",
        ] {
            let label = format!("com.zvuk.kithara-ci.{name}");
            let plist = self.agent_root().join(format!("{label}.plist"));
            if !plist.is_file() {
                continue;
            }
            let _ = self
                .process
                .command("/bin/launchctl")
                .args(["bootout", &format!("{domain}/{label}")])
                .status();
            self.process.run(
                "/bin/launchctl",
                &["bootstrap", &domain, path_text(&plist)?],
                "load CI launch agent",
            )?;
            self.process.run(
                "/bin/launchctl",
                &["enable", &format!("{domain}/{label}")],
                "enable CI launch agent",
            )?;
            self.process.run(
                "/bin/launchctl",
                &["kickstart", "-k", &format!("{domain}/{label}")],
                "start CI launch agent",
            )?;
        }
        info!("CI user services activated");
        Ok(())
    }

    fn runner_config(&self, home: &Path, tokens: &Tokens) -> String {
        let root = self.config.host.host_root.display();
        let url = self.config.host.gitlab_origin();
        let cache = self.config.host.cache_root_linux.display();
        let lane_config = LANE_CONFIG_PATH;
        format!(
            "concurrent = 1\ncheck_interval = 3\nshutdown_timeout = 30\n\n\
             [[runners]]\n  name = \"kithara-mac-mini-linux\"\n  url = \"{url}\"\n  token = \"{}\"\n  executor = \"docker\"\n  builds_dir = \"{root}/workspaces/gitlab\"\n  output_limit = 16384\n  environment = [\"KITHARA_CI_CACHE_ROOT={cache}\", \"KITHARA_CI_HOST_CONFIG={lane_config}\", \"RUSTUP_HOME=/usr/local/rustup\"]\n\
             [runners.docker]\n    host = \"{}\"\n    image = \"{}\"\n    pull_policy = \"if-not-present\"\n    allowed_pull_policies = [\"if-not-present\"]\n    allowed_images = [\"kithara-ci:*\"]\n    cpus = \"5\"\n    memory = \"5g\"\n    privileged = false\n    disable_cache = true\n    shm_size = 1073741824\n    volumes = [\"{root}/cache:{cache}:rw\", \"{root}/cache/gitlab-runner:/cache:rw\", \"{root}/services/host.toml:{lane_config}:ro\"]\n\n\
             [[runners]]\n  name = \"kithara-mac-mini-android\"\n  url = \"{url}\"\n  token = \"{}\"\n  executor = \"shell\"\n  shell = \"bash\"\n  builds_dir = \"{root}/workspaces/gitlab\"\n  output_limit = 16384\n  environment = [\"KITHARA_CI_HOST_CONFIG={lane_config}\"]\n\n\
             [[runners]]\n  name = \"kithara-mac-mini-release\"\n  url = \"{url}\"\n  token = \"{}\"\n  executor = \"shell\"\n  shell = \"bash\"\n  builds_dir = \"{root}/workspaces/gitlab\"\n  output_limit = 16384\n  environment = [\"KITHARA_CI_HOST_CONFIG={lane_config}\"]\n",
            tokens.linux,
            docker_host(home),
            self.config.pins.linux_image,
            tokens.android,
            tokens.release,
        )
    }

    fn install_runner_agents(&self, home: &Path) -> Result<()> {
        let logs = self.config.host.host_root.join("logs");
        let colima = self.config.host.brew_tool("colima").display().to_string();
        let gitlab_runner = self
            .config
            .host
            .brew_tool("gitlab-runner")
            .display()
            .to_string();
        let agent_path = self.config.host.agent_path(home);
        let agents = [
            (
                "colima",
                launchd(
                    "com.zvuk.kithara-ci.colima",
                    &[
                        &colima,
                        "start",
                        "--profile",
                        "kithara",
                        "--foreground",
                        "--cpus",
                        "5",
                        "--memory",
                        "5",
                        "--disk",
                        "100",
                        "--vm-type",
                        "vz",
                        "--vz-rosetta",
                        "--mount-type",
                        "virtiofs",
                    ],
                    &logs.join("colima.log"),
                    &agent_path,
                    "<key>KeepAlive</key><true/><key>ProcessType</key><string>Background</string>",
                ),
            ),
            (
                "gitlab-runner",
                launchd(
                    "com.zvuk.kithara-ci.gitlab-runner",
                    &[
                        &gitlab_runner,
                        "run",
                        "--config",
                        &home
                            .join(".gitlab-runner/config.toml")
                            .display()
                            .to_string(),
                        "--working-directory",
                        &self
                            .config
                            .host
                            .host_root
                            .join("workspaces/gitlab")
                            .display()
                            .to_string(),
                    ],
                    &logs.join("gitlab-runner.log"),
                    &agent_path,
                    "<key>KeepAlive</key><true/><key>ProcessType</key><string>Background</string>\
                     <key>SoftResourceLimits</key><dict>\
                     <key>NumberOfFiles</key><integer>65536</integer></dict>",
                ),
            ),
            (
                "macos-runner",
                launchd(
                    "com.zvuk.kithara-ci.macos-runner",
                    &[
                        &self
                            .config
                            .host
                            .host_root
                            .join("services/bin/kithara-ci")
                            .display()
                            .to_string(),
                        "ci",
                        "host",
                        "--config",
                        &self
                            .config
                            .host
                            .host_root
                            .join("services/host.toml")
                            .display()
                            .to_string(),
                        "--pins",
                        &self
                            .config
                            .host
                            .host_root
                            .join("services/pins.toml")
                            .display()
                            .to_string(),
                        "run-macos-runner",
                    ],
                    &logs.join("macos-runner.log"),
                    &agent_path,
                    "<key>KeepAlive</key><true/><key>ProcessType</key><string>Interactive</string>",
                ),
            ),
        ];
        fs::create_dir_all(self.agent_root())?;
        for (name, contents) in agents {
            let path = self
                .agent_root()
                .join(format!("com.zvuk.kithara-ci.{name}.plist"));
            fs::write(&path, contents)
                .with_context(|| format!("writing runner agent {}", path.display()))?;
        }
        Ok(())
    }

    fn copy_shared_tool(&self, name: &str) -> Result<()> {
        replace_file(
            &self.config.host.brew_tool(name),
            &self
                .config
                .host
                .host_root
                .join("toolchains/shared-bin")
                .join(name),
        )
        .with_context(|| format!("installing shared {name}"))
    }

    pub(super) fn require_ci_user(&self) -> Result<()> {
        let user = self
            .process
            .capture("/usr/bin/id", &["-un"], "current user")?;
        if user != self.config.host.ci_user {
            bail!(
                "run this command as {}, without sudo",
                self.config.host.ci_user
            );
        }
        Ok(())
    }

    pub(super) fn ci_home(&self) -> PathBuf {
        self.config
            .host
            .host_root
            .join("home")
            .join(&self.config.host.ci_user)
    }

    fn agent_root(&self) -> PathBuf {
        self.ci_home().join("Library/LaunchAgents")
    }
}

pub(super) fn docker_host(home: &Path) -> String {
    format!(
        "unix://{}",
        home.join(".colima/kithara/docker.sock").display()
    )
}

pub(super) struct Tokens {
    pub(super) macos: String,
    linux: String,
    android: String,
    release: String,
}

impl Tokens {
    pub(super) fn load(root: &Path) -> Result<Self> {
        Ok(Self {
            macos: read_token(root, "macos")?,
            linux: read_token(root, "linux")?,
            android: read_token(root, "android")?,
            release: read_token(root, "release")?,
        })
    }
}

fn read_token(root: &Path, name: &str) -> Result<String> {
    let path = root.join(format!("runner-{name}.token"));
    let token = read_secret(&path)?;
    if !token.starts_with("glrt-") || token.chars().any(char::is_whitespace) {
        bail!("invalid runner authentication token in {}", path.display());
    }
    Ok(token)
}

pub(super) fn read_secret(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("reading metadata for {}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        bail!("secret path must be a regular file: {}", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if metadata.permissions().mode() & 0o077 != 0 {
            bail!("secret file must have mode 0600: {}", path.display());
        }
    }
    read_trimmed(path)
}

pub(super) fn read_trimmed(path: &Path) -> Result<String> {
    let value = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("file is empty: {}", path.display());
    }
    Ok(trimmed.to_owned())
}

pub(super) fn write_secure(path: &Path, contents: &str) -> Result<()> {
    fs::write(path, contents).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("securing {}", path.display()))?;
    }
    Ok(())
}

pub(super) fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("path is not UTF-8: {}", path.display()))
}

/// Provisioning reruns, and some sources are mode `0555` (Homebrew binaries),
/// so the destination cannot be reopened for writing. Replace, never overwrite.
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        fs::remove_file(destination)
            .with_context(|| format!("removing stale {}", destination.display()))?;
    }
    fs::copy(source, destination)
        .with_context(|| format!("copying into {}", destination.display()))?;
    Ok(())
}

pub(super) fn require_macos() -> Result<()> {
    if std::env::consts::OS != "macos" {
        bail!("runner host command supports macOS only");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::ci::config::fixture;

    #[test]
    fn rendered_runner_configs_are_valid_toml_and_yaml() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let config = fixture();
        let process = Process::new(&root, BTreeMap::new());
        let manager = RunnerManager::new(&config, &process);
        let home = config
            .host
            .host_root
            .join("home")
            .join(&config.host.ci_user);
        let tokens = Tokens {
            macos: "glrt-macos".into(),
            linux: "glrt-linux".into(),
            android: "glrt-android".into(),
            release: "glrt-release".into(),
        };

        toml::from_str::<toml::Value>(&manager.runner_config(&home, &tokens)).unwrap();
    }

    #[test]
    fn generated_configs_trust_the_platform_store_only() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let config = fixture();
        let process = Process::new(&root, BTreeMap::new());
        let manager = RunnerManager::new(&config, &process);
        let home = config
            .host
            .host_root
            .join("home")
            .join(&config.host.ci_user);
        let tokens = Tokens {
            macos: "glrt-macos".into(),
            linux: "glrt-linux".into(),
            android: "glrt-android".into(),
            release: "glrt-release".into(),
        };

        let runner = manager.runner_config(&home, &tokens);
        for rendered in [&runner] {
            for forbidden in [
                "tls-ca-file",
                "tls_ca_file",
                "ca.crt",
                "kithara-certs",
                "shared-certs",
                "tls-skip-verify",
                "insecure",
            ] {
                assert!(
                    !rendered.contains(forbidden),
                    "rendered CI configuration must not carry {forbidden}"
                );
            }
        }
        assert!(runner.contains(&config.host.gitlab_origin()));
    }

    /// Homebrew ships tools as mode `0555`, so a plain `fs::copy` onto a
    /// previous provisioning pass fails with EACCES.
    #[cfg(unix)]
    #[test]
    fn replacing_a_read_only_file_succeeds() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        fs::write(&source, "new").unwrap();
        fs::write(&destination, "old").unwrap();
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o555)).unwrap();

        replace_file(&source, &destination).unwrap();
        assert_eq!(fs::read_to_string(&destination).unwrap(), "new");
    }

    #[cfg(unix)]
    #[test]
    fn secret_files_must_not_be_group_or_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("secret");
        fs::write(&path, "value\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_secret(&path).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_secret(&path).unwrap(), "value");
    }
}
