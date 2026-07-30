use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Serialize;
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
        let ca = config_root.join("gitlab-ca.crt");
        let image_digest_path = config_root.join("linux-image.digest");
        let cilicon_password_path = config_root.join("cilicon-ssh.password");
        if !ca.is_file() {
            bail!("missing corporate GitLab CA: {}", ca.display());
        }
        let expected_linux = read_trimmed(&image_digest_path)?;
        let actual_linux = self.linux_image_digest(&home)?;
        if expected_linux != actual_linux {
            bail!("Linux CI image digest changed: expected {expected_linux}, found {actual_linux}");
        }
        let tokens = Tokens::load(&config_root)?;
        let cilicon_password = read_secret(&cilicon_password_path)?;
        for path in [
            &config_root,
            &runner_root,
            &runner_root.join("certs"),
            &self.config.host.host_root.join("cache/gitlab-runner"),
            &self.config.host.host_root.join("toolchains/shared-certs"),
            &self.config.host.host_root.join("toolchains/shared-bin"),
            &self.config.host.host_root.join("vm/cilicon-clone"),
            &self.config.host.host_root.join("workspaces/gitlab"),
            &self.agent_root(),
        ] {
            fs::create_dir_all(path)
                .with_context(|| format!("creating runner directory {}", path.display()))?;
        }
        let ca_name = format!("{}.crt", self.config.host.gitlab_host()?);
        fs::copy(&ca, runner_root.join("certs").join(&ca_name))
            .context("installing GitLab runner CA")?;
        fs::copy(
            &ca,
            self.config
                .host
                .host_root
                .join("toolchains/shared-certs")
                .join(&ca_name),
        )
        .context("installing shared GitLab CA")?;
        self.copy_shared_tool("xcodegen")?;
        let current = std::env::current_exe().context("resolving CI executable")?;
        fs::copy(
            current,
            self.config
                .host
                .host_root
                .join("toolchains/shared-bin/kithara-ci"),
        )
        .context("installing CI executable for Cilicon guests")?;
        for name in ["host.json", "pins.json"] {
            fs::copy(
                self.config.host.host_root.join("services").join(name),
                self.config
                    .host
                    .host_root
                    .join("toolchains/shared-bin")
                    .join(name),
            )
            .with_context(|| format!("installing Cilicon guest {name}"))?;
        }

        write_secure(
            &runner_root.join("config.toml"),
            &self.runner_config(&home, &runner_root, &tokens)?,
        )?;
        write_secure(
            &home.join("cilicon.yml"),
            &self.cilicon_config(&home, &tokens, &cilicon_password)?,
        )?;
        write_secure(
            &config_root.join("cilicon-image.digest"),
            &format!("{}\n", self.config.pins.cilicon_image_digest),
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
        for name in ["cleanup", "health", "colima", "gitlab-runner", "cilicon"] {
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

    fn runner_config(&self, home: &Path, runner_root: &Path, tokens: &Tokens) -> Result<String> {
        let root = self.config.host.host_root.display();
        let url = self.config.host.gitlab_origin();
        let cache = self.config.host.cache_root_linux.display();
        let lane_config = LANE_CONFIG_PATH;
        let ca = runner_root
            .join("certs")
            .join(format!("{}.crt", self.config.host.gitlab_host()?));
        Ok(format!(
            "concurrent = 1\ncheck_interval = 3\nshutdown_timeout = 30\n\n\
             [[runners]]\n  name = \"kithara-mac-mini-linux\"\n  url = \"{url}\"\n  token = \"{}\"\n  tls-ca-file = \"{}\"\n  executor = \"docker\"\n  builds_dir = \"{root}/workspaces/gitlab\"\n  output_limit = 16384\n  environment = [\"KITHARA_CI_CACHE_ROOT={cache}\", \"KITHARA_CI_HOST_CONFIG={lane_config}\", \"RUSTUP_HOME=/usr/local/rustup\"]\n\
             [runners.docker]\n    host = \"{}\"\n    image = \"{}\"\n    pull_policy = \"if-not-present\"\n    allowed_pull_policies = [\"if-not-present\"]\n    allowed_images = [\"kithara-ci:*\"]\n    cpus = \"5\"\n    memory = \"5g\"\n    privileged = false\n    disable_cache = true\n    shm_size = 1073741824\n    volumes = [\"{root}/cache:{cache}:rw\", \"{root}/cache/gitlab-runner:/cache:rw\", \"{root}/services/host.json:{lane_config}:ro\"]\n\n\
             [[runners]]\n  name = \"kithara-mac-mini-android\"\n  url = \"{url}\"\n  token = \"{}\"\n  tls-ca-file = \"{}\"\n  executor = \"shell\"\n  shell = \"bash\"\n  builds_dir = \"{root}/workspaces/gitlab\"\n  output_limit = 16384\n  environment = [\"KITHARA_CI_HOST_CONFIG={lane_config}\"]\n\n\
             [[runners]]\n  name = \"kithara-mac-mini-release\"\n  url = \"{url}\"\n  token = \"{}\"\n  tls-ca-file = \"{}\"\n  executor = \"shell\"\n  shell = \"bash\"\n  builds_dir = \"{root}/workspaces/gitlab\"\n  output_limit = 16384\n  environment = [\"KITHARA_CI_HOST_CONFIG={lane_config}\"]\n",
            tokens.linux,
            ca.display(),
            docker_host(home),
            self.config.pins.linux_image,
            tokens.android,
            ca.display(),
            tokens.release,
            ca.display(),
        ))
    }

    fn cilicon_config(&self, home: &Path, tokens: &Tokens, ssh_password: &str) -> Result<String> {
        let root = &self.config.host.host_root;
        let shared = self.config.host.cilicon_guest_shared_root.display();
        let pre_run = format!(
            "\"{shared}/kithara-tools/kithara-ci\" ci host --config \
             \"{shared}/kithara-tools/host.json\" --pins \
             \"{shared}/kithara-tools/pins.json\" guest-prepare"
        );
        let config = CiliconConfig {
            source: &self.config.pins.cilicon_image,
            runner_name: "kithara-mac-mini-macos",
            vm_clone_path: root.join("vm/cilicon-clone"),
            retry_delay: 30,
            ssh_connect_max_retries: 30,
            console_devices: ["tart-version-2"],
            ssh_credentials: SshCredentials {
                username: &self.config.host.cilicon_ssh_user,
                password: ssh_password,
            },
            hardware: Hardware {
                cpu_cores: 8,
                ram_gigabytes: 12,
                connects_to_audio_device: false,
                display: Display {
                    width: 1_440,
                    height: 900,
                    pixels_per_inch: 80,
                },
            },
            directory_mounts: [
                DirectoryMount {
                    host_path: root.join("cache"),
                    guest_folder: "kithara-cache",
                    read_only: false,
                },
                DirectoryMount {
                    host_path: home.join(".cargo"),
                    guest_folder: "kithara-cargo",
                    read_only: true,
                },
                DirectoryMount {
                    host_path: home.join(".rustup"),
                    guest_folder: "kithara-rustup",
                    read_only: true,
                },
                DirectoryMount {
                    host_path: root.join("toolchains/shared-bin"),
                    guest_folder: "kithara-tools",
                    read_only: true,
                },
                DirectoryMount {
                    host_path: root.join("toolchains/shared-certs"),
                    guest_folder: "kithara-certs",
                    read_only: true,
                },
            ],
            pre_run: &pre_run,
            provisioner: Provisioner {
                kind: "gitlab",
                config: GitlabProvisioner {
                    gitlab_url: self.config.host.gitlab_origin(),
                    runner_token: &tokens.macos,
                    executor: "shell",
                    max_number_of_builds: 1,
                    download_latest: true,
                    download_url: format!(
                        "https://gitlab-runner-downloads.s3.amazonaws.com/v{}/binaries/\
                         gitlab-runner-darwin-arm64",
                        self.config.pins.gitlab_runner_version
                    ),
                },
            },
        };
        serde_yml::to_string(&config).context("serializing Cilicon configuration")
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
                    "<key>KeepAlive</key><true/><key>ProcessType</key><string>Background</string>",
                ),
            ),
            (
                "cilicon",
                launchd(
                    "com.zvuk.kithara-ci.cilicon",
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
                            .join("services/host.json")
                            .display()
                            .to_string(),
                        "--pins",
                        &self
                            .config
                            .host
                            .host_root
                            .join("services/pins.json")
                            .display()
                            .to_string(),
                        "start-cilicon",
                    ],
                    &logs.join("cilicon.log"),
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
        fs::copy(
            self.config.host.brew_tool(name),
            self.config
                .host
                .host_root
                .join("toolchains/shared-bin")
                .join(name),
        )
        .with_context(|| format!("installing shared {name}"))?;
        Ok(())
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

struct Tokens {
    macos: String,
    linux: String,
    android: String,
    release: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CiliconConfig<'a> {
    source: &'a str,
    runner_name: &'a str,
    vm_clone_path: PathBuf,
    retry_delay: u64,
    ssh_connect_max_retries: u64,
    console_devices: [&'a str; 1],
    ssh_credentials: SshCredentials<'a>,
    hardware: Hardware,
    directory_mounts: [DirectoryMount<'a>; 5],
    pre_run: &'a str,
    provisioner: Provisioner<'a>,
}

#[derive(Serialize)]
struct SshCredentials<'a> {
    username: &'a str,
    password: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Hardware {
    cpu_cores: u8,
    ram_gigabytes: u8,
    connects_to_audio_device: bool,
    display: Display,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Display {
    width: u16,
    height: u16,
    pixels_per_inch: u8,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectoryMount<'a> {
    host_path: PathBuf,
    guest_folder: &'a str,
    read_only: bool,
}

#[derive(Serialize)]
struct Provisioner<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    config: GitlabProvisioner<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GitlabProvisioner<'a> {
    #[serde(rename = "gitlabURL")]
    gitlab_url: String,
    runner_token: &'a str,
    executor: &'a str,
    max_number_of_builds: u8,
    download_latest: bool,
    #[serde(rename = "downloadURL")]
    download_url: String,
}

impl Tokens {
    fn load(root: &Path) -> Result<Self> {
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

fn read_secret(path: &Path) -> Result<String> {
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
        let runner_root = home.join(".gitlab-runner");
        let tokens = Tokens {
            macos: "glrt-macos".into(),
            linux: "glrt-linux".into(),
            android: "glrt-android".into(),
            release: "glrt-release".into(),
        };

        toml::from_str::<toml::Value>(
            &manager.runner_config(&home, &runner_root, &tokens).unwrap(),
        )
        .unwrap();
        let cilicon = manager
            .cilicon_config(&home, &tokens, "fixture-password")
            .unwrap();
        serde_yml::from_str::<serde_yml::Value>(&cilicon).unwrap();
        assert!(cilicon.contains("gitlabURL:"));
        assert!(cilicon.contains("downloadURL:"));
        assert!(cilicon.contains("preRun:"));
        assert!(!cilicon.contains("password: admin"));
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
