use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use tracing::info;

use super::services::launchd;
use crate::ci::{
    config::{CiConfig, MAC_CONFIG_PATH},
    environment::PROVISIONED_LINUX_IMAGE_ENV,
    process::Process,
};

pub(super) struct RunnerManager<'a> {
    pub(super) config: &'a CiConfig,
    pub(super) process: &'a Process,
}

impl<'a> RunnerManager<'a> {
    pub(super) const fn new(config: &'a CiConfig, process: &'a Process) -> Self {
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
        for name in ["mac-host.toml", "pins.toml"] {
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
        if !self.launchctl_knows(&domain) {
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
            let service = format!("{domain}/{label}");
            let _ = self
                .process
                .command("/bin/launchctl")
                .args(["bootout", &service])
                .status();
            self.await_unload(&service)?;
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

    /// `launchctl print` answers for a domain or a loaded service and fails
    /// otherwise. Its output is enormous, so keep it off the console.
    fn launchctl_knows(&self, target: &str) -> bool {
        self.process
            .command("/bin/launchctl")
            .args(["print", target])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// launchd tears a service down asynchronously, and bootstrapping one
    /// that is still on its way out fails with `EIO` — which would leave the
    /// service unloaded and its process stopped.
    fn await_unload(&self, service: &str) -> Result<()> {
        const ATTEMPTS: u32 = 120;
        const POLL: Duration = Duration::from_millis(500);
        for _ in 0..ATTEMPTS {
            if !self.launchctl_knows(service) {
                return Ok(());
            }
            thread::sleep(POLL);
        }
        bail!("{service} is still loaded after being booted out")
    }

    /// The Linux container's ceiling is deliberately below the VM's eight
    /// gigabytes, so a runaway job cannot take the VM down with it. Five was
    /// too low: one `rustc` compiling the largest test crate reached 3.3
    /// gibibytes on its own and the kernel killed it, which Cargo reported
    /// only as "could not compile" with no diagnostic at all.
    fn runner_config(&self, home: &Path, tokens: &Tokens) -> String {
        let root = self.config.host.host_root.display();
        let url = self.config.host.gitlab_origin();
        let cache = self.config.host.cache_root_linux.display();
        let lane_config = MAC_CONFIG_PATH;
        let image = &self.config.pins.linux_image;
        let provisioned_image = format!("{PROVISIONED_LINUX_IMAGE_ENV}={image}");
        format!(
            "concurrent = 1\ncheck_interval = 3\nshutdown_timeout = 30\n\n\
             [[runners]]\n  name = \"kithara-mac-mini-linux\"\n  url = \"{url}\"\n  token = \"{}\"\n  executor = \"docker\"\n  builds_dir = \"{root}/workspaces/gitlab\"\n  output_limit = 16384\n  environment = [\"KITHARA_CI_CACHE_ROOT={cache}\", \"KITHARA_CI_HOST_CONFIG={lane_config}\", \"{provisioned_image}\", \"RUSTUP_HOME=/usr/local/rustup\"]\n\
             [runners.docker]\n    host = \"{}\"\n    image = \"{image}\"\n    pull_policy = \"never\"\n    allowed_pull_policies = [\"never\"]\n    allowed_images = [\"{image}\"]\n    cpus = \"5\"\n    memory = \"6500m\"\n    privileged = false\n    disable_cache = true\n    shm_size = 1073741824\n    volumes = [\"{root}/cache:{cache}:rw\", \"{root}/cache/gitlab-runner:/cache:rw\", \"{root}/services/mac-host.toml:{lane_config}:ro\"]\n\n\
             [[runners]]\n  name = \"kithara-mac-mini-android\"\n  url = \"{url}\"\n  token = \"{}\"\n  executor = \"shell\"\n  shell = \"bash\"\n  builds_dir = \"{root}/workspaces/gitlab\"\n  output_limit = 16384\n  environment = [\"KITHARA_CI_CACHE_ROOT={root}/cache\", \"KITHARA_CI_HOST_CONFIG={lane_config}\"]\n\n\
             [[runners]]\n  name = \"kithara-mac-mini-release\"\n  url = \"{url}\"\n  token = \"{}\"\n  executor = \"shell\"\n  shell = \"bash\"\n  builds_dir = \"{root}/workspaces/gitlab\"\n  output_limit = 16384\n  environment = [\"KITHARA_CI_CACHE_ROOT={root}/cache\", \"KITHARA_CI_HOST_CONFIG={lane_config}\"]\n",
            tokens.linux,
            docker_host(home),
            tokens.android,
            tokens.release,
        )
    }

    /// A bind mount is resolved by the Docker daemon, which lives inside
    /// colima's virtual machine — not on the Mac. colima mounts the CI home
    /// and nothing else, so every other source the runner binds was missing
    /// there, and Docker substitutes an empty directory for a source it cannot
    /// find rather than refusing. That is how the Linux lane ran with no host
    /// profile and no shared cache, reporting only that the profile it was
    /// handed had somehow become a directory.
    ///
    /// The mounts name siblings of the CI home rather than the volume root:
    /// colima already mounts the home, and lima rejects one mount nested
    /// inside another.
    fn colima_args(&self, colima: &str) -> Vec<String> {
        let root = &self.config.host.host_root;
        let mut args: Vec<String> = [
            colima,
            "start",
            "--profile",
            "kithara",
            "--foreground",
            "--cpus",
            "5",
            // Linking is this workspace's memory peak, not compiling it. At
            // five gigabytes the kernel killed `ld` outright in both the test
            // and the coverage lane, leaving only "terminated with signal 9"
            // behind. The macOS guest keeps twelve of the host's twenty-four
            // and the two rarely peak together.
            "--memory",
            "8",
            "--disk",
            "100",
            "--vm-type",
            "vz",
            "--vz-rosetta",
            "--mount-type",
            "virtiofs",
        ]
        .iter()
        .map(|value| (*value).to_string())
        .collect();
        for mount in [
            format!("{}:w", root.join("cache").display()),
            format!("{}:w", root.join("services").display()),
        ] {
            args.push("--mount".to_string());
            args.push(mount);
        }
        args
    }

    fn install_runner_agents(&self, home: &Path) -> Result<()> {
        let logs = self.config.host.host_root.join("logs");
        let colima = self.config.host.brew_tool("colima").display().to_string();
        let colima_args = self.colima_args(&colima);
        let colima_args: Vec<&str> = colima_args.iter().map(String::as_str).collect();
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
                    &colima_args,
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
                        // The copy `configure-runners` installs, not the
                        // root-owned one. This agent's own plist already lives
                        // in the CI user's home, so requiring root to replace
                        // the binary it names protects nothing and blocks every
                        // routine update of the runner loop.
                        &self
                            .config
                            .host
                            .host_root
                            .join("toolchains/shared-bin/kithara-ci")
                            .display()
                            .to_string(),
                        "ci",
                        "host",
                        "--config",
                        &self
                            .config
                            .host
                            .host_root
                            .join("services/mac-host.toml")
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
/// so the destination cannot be reopened for writing. Stage beside it and
/// rename over it: renaming needs no write permission on the file itself, and
/// it never leaves the destination missing — clearing it first destroyed the
/// only copy whenever a command installed the executable it was running from.
///
/// Renaming is also what keeps a signed executable runnable. macOS validates a
/// signature once per inode and caches the verdict; rewriting the bytes in
/// place leaves that verdict attached to content it no longer describes, and
/// the kernel answers the next exec with SIGKILL. A rename installs a new
/// inode, so the next exec is validated afresh.
pub(super) fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    let name = destination
        .file_name()
        .with_context(|| format!("no file name in destination {}", destination.display()))?;
    let mut staged = name.to_os_string();
    staged.push(format!(".incoming.{}", std::process::id()));
    let staged = destination.with_file_name(staged);

    fs::copy(source, &staged).with_context(|| {
        format!(
            "staging {} beside {}",
            source.display(),
            destination.display()
        )
    })?;
    fs::rename(&staged, destination)
        .inspect_err(|_| {
            let _ = fs::remove_file(&staged);
        })
        .with_context(|| format!("installing {}", destination.display()))
}

pub(super) fn require_macos() -> Result<()> {
    if std::env::consts::OS != "macos" {
        bail!("runner host command supports macOS only");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, os::unix::fs::PermissionsExt};

    use super::*;
    use crate::ci::config::fixture;

    #[test]
    fn installing_an_executable_over_itself_keeps_it() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("kithara-ci");
        fs::write(&path, b"payload").expect("seed the destination");

        replace_file(&path, &path).expect("install in place");

        assert_eq!(fs::read(&path).expect("read the destination"), b"payload");
    }

    #[test]
    fn installing_over_a_read_only_destination_replaces_it() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        fs::write(&source, b"new").expect("seed the source");
        fs::write(&destination, b"old").expect("seed the destination");
        fs::set_permissions(&destination, PermissionsExt::from_mode(0o555))
            .expect("make the destination read-only");

        replace_file(&source, &destination).expect("install over a read-only file");

        assert_eq!(
            fs::read(&destination).expect("read the destination"),
            b"new"
        );
    }

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

        let rendered: toml::Value = toml::from_str(&manager.runner_config(&home, &tokens)).unwrap();
        let linux = rendered["runners"]
            .as_array()
            .unwrap()
            .iter()
            .find(|runner| runner["name"].as_str() == Some("kithara-mac-mini-linux"))
            .unwrap();
        assert_eq!(
            linux["docker"]["image"].as_str(),
            Some(config.pins.linux_image.as_str())
        );
        assert_eq!(linux["docker"]["pull_policy"].as_str(), Some("never"));
        assert_eq!(
            linux["docker"]["allowed_pull_policies"][0].as_str(),
            Some("never")
        );
        assert_eq!(
            linux["docker"]["allowed_images"][0].as_str(),
            Some(config.pins.linux_image.as_str())
        );
        assert!(
            linux["environment"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| {
                    value.as_str().is_some_and(|entry| {
                        entry
                            == format!("{PROVISIONED_LINUX_IMAGE_ENV}={}", config.pins.linux_image)
                    })
                })
        );
    }

    /// The runner config and the colima agent are written by two different
    /// functions and read by two different programs; nothing but this test
    /// says they have to agree. When they stopped agreeing, Docker filled the
    /// gap with empty directories and the lane failed several steps later,
    /// describing a file as a directory.
    #[test]
    fn colima_mounts_every_source_the_docker_runner_binds() {
        let config = fixture();
        let process = Process::new(Path::new("/"), BTreeMap::new());
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
        let rendered: toml::Value =
            toml::from_str(&manager.runner_config(&home, &tokens)).expect("runner config is TOML");
        // The pipeline builds `SCCACHE_DIR` out of this, so a runner that
        // leaves it unset would resolve the compiler cache against the
        // filesystem root and fail every build on that executor.
        for runner in rendered["runners"].as_array().expect("runners is an array") {
            let environment = runner["environment"]
                .as_array()
                .expect("every runner declares an environment");
            assert!(
                environment
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .any(|entry| entry.starts_with("KITHARA_CI_CACHE_ROOT=")),
                "{} does not name a cache root",
                runner["name"]
            );
        }

        let volumes = rendered["runners"]
            .as_array()
            .expect("runners is an array")
            .iter()
            .filter_map(|runner| runner.get("docker")?.get("volumes")?.as_array())
            .flatten()
            .filter_map(toml::Value::as_str);

        let args = manager.colima_args("colima");
        let mounts: Vec<&str> = args
            .windows(2)
            .filter(|pair| pair[0] == "--mount")
            .map(|pair| pair[1].trim_end_matches(":w"))
            .collect();
        assert!(!mounts.is_empty(), "the agent declares no mounts");

        let mut checked = 0;
        for volume in volumes {
            let source = volume.split(':').next().expect("a volume has a source");
            if !source.starts_with('/') || source.starts_with(home.to_str().expect("UTF-8 home")) {
                continue;
            }
            assert!(
                mounts.iter().any(|mount| source.starts_with(mount)),
                "{source} is bound into containers but colima does not mount it"
            );
            checked += 1;
        }
        assert!(checked > 0, "no host-side bind sources were checked");
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
