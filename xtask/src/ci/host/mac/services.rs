#[cfg(unix)]
use std::{
    ffi::OsStr,
    fs::{File, Permissions},
    os::unix::fs::{MetadataExt, PermissionsExt, fchown},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
#[cfg(unix)]
use kithara_devtools::lock;
#[cfg(unix)]
use nix::{
    errno::Errno,
    fcntl::{AtFlags, OFlag, open, openat},
    sys::stat::{Mode, fstat, fstatat, mkdirat},
};
use tracing::info;

use crate::{
    ci::{bridge, config::CiConfig, process::Process},
    consts,
};

pub(super) struct ServiceInstaller<'a> {
    config: &'a CiConfig,
    process: &'a Process,
}

impl<'a> ServiceInstaller<'a> {
    const BINARY_NAME: &'static str = "kithara-ci";
    const HOST_CONFIG_NAME: &'static str = "mac-host.toml";
    const PINS_NAME: &'static str = "pins.toml";

    pub(super) const fn new(config: &'a CiConfig, process: &'a Process) -> Self {
        Self { config, process }
    }

    pub(super) fn install(&self, config_path: &Path, pins_path: &Path) -> Result<()> {
        require_macos()?;
        self.require_root()?;
        self.ensure_sync_user()?;
        self.ensure_layout()?;
        self.install_binary()?;
        self.install_config(config_path, pins_path)?;
        self.install_maintenance_agents()?;
        self.stage_bridge_agent()?;
        info!(
            binary = %self.installed_binary().display(),
            "root-owned CI services installed"
        );
        Ok(())
    }

    pub(super) fn activate_bridge(&self) -> Result<()> {
        require_macos()?;
        self.require_root()?;
        let bridge = self.service_root().join("bridge");
        let config = bridge.join("config.toml");
        let pending = bridge.join("com.zvuk.kithara-ci.bridge.plist.pending");
        if !config.is_file() {
            bail!("missing bridge configuration: {}", config.display());
        }
        if !pending.is_file() {
            bail!("missing staged bridge service: {}", pending.display());
        }
        self.require_private_sync_file(&config)?;
        for secret in bridge::secret_files(&config)? {
            self.require_private_sync_file(&secret)?;
        }
        let binary = self.installed_binary();
        self.process.run(
            path_text(&binary)?,
            &["ci", "bridge", "validate", "--config", path_text(&config)?],
            "validate repository bridge configuration",
        )?;
        let destination = Path::new("/Library/LaunchDaemons/com.zvuk.kithara-ci.bridge.plist");
        self.install_file(&pending, destination, "0644", "root")?;
        let service = "system/com.zvuk.kithara-ci.bridge";
        let _ = self
            .process
            .command("/bin/launchctl")
            .args(["bootout", service])
            .status();
        self.process.run(
            "/bin/launchctl",
            &["bootstrap", "system", path_text(destination)?],
            "activate repository bridge",
        )?;
        self.process.run(
            "/bin/launchctl",
            &["enable", service],
            "enable repository bridge",
        )?;
        self.process.run(
            "/bin/launchctl",
            &["kickstart", "-k", service],
            "start repository bridge",
        )?;
        info!("repository bridge activated");
        Ok(())
    }

    fn require_root(&self) -> Result<()> {
        let uid = self
            .process
            .capture("/usr/bin/id", &["-u"], "current user id")?;
        if uid != "0" {
            bail!("run this command with sudo");
        }
        Ok(())
    }

    fn ensure_sync_user(&self) -> Result<()> {
        let user = &self.config.host.sync_user;
        let uid = self.config.host.sync_uid.to_string();
        let home = self.sync_home();
        let owner = self
            .process
            .capture(
                "/usr/bin/dscl",
                &[".", "-search", "/Users", "UniqueID", &uid],
                "lookup synchronization UID",
            )
            .unwrap_or_default();
        if let Some(existing) = owner.split_whitespace().next()
            && existing != user
        {
            bail!(
                "UID {} already belongs to {existing}",
                self.config.host.sync_uid
            );
        }
        let exists = self
            .process
            .command("/usr/bin/id")
            .arg(user)
            .status()
            .is_ok_and(|status| status.success());
        if !exists {
            let record = format!("/Users/{user}");
            let home = home.to_str().context("synchronization home is not UTF-8")?;
            for args in [
                vec![".", "-create", &record],
                vec![
                    ".",
                    "-create",
                    &record,
                    "RealName",
                    "Kithara Repository Sync",
                ],
                vec![".", "-create", &record, "UniqueID", &uid],
                vec![".", "-create", &record, "PrimaryGroupID", "20"],
                vec![".", "-create", &record, "NFSHomeDirectory", home],
                vec![".", "-create", &record, "UserShell", "/usr/bin/false"],
                vec![".", "-create", &record, "IsHidden", "1"],
            ] {
                let mut command = self.process.command("/usr/bin/dscl");
                command.args(args);
                self.process
                    .run_command(&mut command, "create synchronization user")?;
            }
        }
        let actual_uid =
            self.process
                .capture("/usr/bin/id", &["-u", user], "verify synchronization user")?;
        if actual_uid != uid {
            bail!("{user} has UID {actual_uid}, expected {uid}");
        }
        Ok(())
    }

    fn ensure_layout(&self) -> Result<()> {
        self.install_cleanup_lock()?;
        for (path, mode, owner) in [
            (self.service_root().join("bin"), "0755", "root"),
            (self.service_root().join("bridge"), "0755", "root"),
            (
                self.service_root().join("bridge/secrets"),
                "0700",
                self.config.host.sync_user.as_str(),
            ),
            (
                self.service_root().join("bridge/state"),
                "0700",
                self.config.host.sync_user.as_str(),
            ),
            (
                self.sync_home(),
                "0700",
                self.config.host.sync_user.as_str(),
            ),
            (self.agent_root(), "0755", self.config.host.ci_user.as_str()),
        ] {
            self.install_directory(&path, mode, owner)?;
        }
        let log = self.config.host.host_root.join("logs/bridge.log");
        if !log.exists() {
            self.install_empty_file(&log, "0640", &self.config.host.sync_user)?;
        }
        Ok(())
    }

    #[cfg(unix)]
    fn install_cleanup_lock(&self) -> Result<()> {
        let (host, directory, file) = self.open_cleanup_lock()?;
        let uid = self
            .process
            .capture(
                "/usr/bin/id",
                &["-u", &self.config.host.ci_user],
                "lookup CI user id",
            )?
            .parse::<u32>()
            .context("invalid CI user id")?;
        let group = self.process.capture(
            "/usr/bin/dscl",
            &[".", "-read", "/Groups/staff", "PrimaryGroupID"],
            "lookup staff group id",
        )?;
        let gid = group
            .strip_prefix("PrimaryGroupID: ")
            .context("invalid staff group record")?
            .parse::<u32>()
            .context("invalid staff group id")?;
        // fchown treats all-bits-one IDs as "leave unchanged".
        if uid == u32::MAX || gid == u32::MAX {
            bail!("cleanup lock ownership must name a user and group");
        }
        self.set_cleanup_lock_metadata(&host, &directory, &file, (uid, gid))
    }

    #[cfg(not(unix))]
    fn install_cleanup_lock(&self) -> Result<()> {
        bail!("cleanup lock installation requires Unix ownership metadata")
    }

    #[cfg(unix)]
    fn open_cleanup_lock(&self) -> Result<(File, File, File)> {
        let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
        let host = File::from(
            open(&self.config.host.host_root, flags, Mode::empty())
                .context("opening CI host root for cleanup lock installation")?,
        );
        match mkdirat(&host, "locks", Mode::from_bits_truncate(0o750)) {
            Ok(()) | Err(Errno::EEXIST) => {}
            Err(error) => return Err(error).context("creating cleanup lock directory"),
        }
        let directory = File::from(
            openat(&host, "locks", flags, Mode::empty())
                .context("opening cleanup lock directory")?,
        );
        let file = lock::open_at(&directory, OsStr::new("cleanup.lock"))
            .context("opening cleanup lock file")?;
        Ok((host, directory, file))
    }

    #[cfg(unix)]
    fn set_cleanup_lock_metadata(
        &self,
        host: &File,
        directory: &File,
        file: &File,
        owner: (u32, u32),
    ) -> Result<()> {
        let metadata = file.metadata().context("reading cleanup lock metadata")?;
        if !directory.metadata()?.is_dir() || !metadata.is_file() || metadata.nlink() != 1 {
            bail!("cleanup lock requires a directory and a singly linked regular file");
        }
        for (handle, mode) in [(directory, 0o750), (file, 0o600)] {
            fchown(handle, Some(owner.0), Some(owner.1))
                .context("setting cleanup lock ownership")?;
            handle
                .set_permissions(Permissions::from_mode(mode))
                .context("setting cleanup lock permissions")?;
        }
        let current_host = File::from(
            open(
                &self.config.host.host_root,
                OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
                Mode::empty(),
            )
            .context("rechecking CI host root after cleanup lock installation")?,
        );
        for (expected, current) in [
            (fstat(host)?, fstat(&current_host)?),
            (
                fstat(directory)?,
                fstatat(host, "locks", AtFlags::AT_SYMLINK_NOFOLLOW)?,
            ),
            (
                fstat(file)?,
                fstatat(directory, "cleanup.lock", AtFlags::AT_SYMLINK_NOFOLLOW)?,
            ),
        ] {
            if expected.st_dev != current.st_dev || expected.st_ino != current.st_ino {
                bail!("cleanup lock path changed during installation");
            }
        }
        Ok(())
    }

    fn install_binary(&self) -> Result<()> {
        let source = std::env::current_exe().context("resolving current CI executable")?;
        self.install_file(&source, &self.installed_binary(), "0755", "root")?;
        let shared = self.config.host.host_root.join("toolchains/shared-bin");
        self.install_directory(&shared, "0755", &self.config.host.ci_user)?;
        self.install_file(
            &source,
            &shared.join(Self::BINARY_NAME),
            "0755",
            &self.config.host.ci_user,
        )
    }

    fn install_config(&self, config_path: &Path, pins_path: &Path) -> Result<()> {
        for (label, path) in [
            ("CI host profile", config_path),
            ("CI build pins", pins_path),
        ] {
            if !path.is_file() {
                bail!("missing {label}: {}", path.display());
            }
        }
        let host = self.installed_config();
        let pins = self.installed_pins();
        self.config.host.write(&host)?;
        self.config.pins.write(&pins)?;
        let shared = self.config.host.host_root.join("toolchains/shared-bin");
        for (source, name) in [(&host, Self::HOST_CONFIG_NAME), (&pins, Self::PINS_NAME)] {
            self.process.run(
                "/usr/sbin/chown",
                &["root:wheel", path_text(source)?],
                "set installed CI config ownership",
            )?;
            self.process.run(
                "/bin/chmod",
                &["0644", path_text(source)?],
                "set installed CI config permissions",
            )?;
            self.install_file(
                source,
                &shared.join(name),
                "0644",
                &self.config.host.ci_user,
            )?;
        }
        self.install_directory(Path::new(consts::LANE_CONFIG_DIR), "0755", "root")?;
        self.install_file(&host, Path::new(consts::MAC_CONFIG_PATH), "0644", "root")
    }

    fn install_maintenance_agents(&self) -> Result<()> {
        let binary = self.replaceable_binary().display().to_string();
        let config = self.installed_config().display().to_string();
        let pins = self.installed_pins().display().to_string();
        let logs = &self.config.host.host_root.join("logs");
        let path = self.config.host.agent_path(&self.ci_home());
        let cleanup = launchd(
            "com.zvuk.kithara-ci.cleanup",
            &[
                &binary, "ci", "host", "mac", "--config", &config, "--pins", &pins, "cleanup",
            ],
            &logs.join("cleanup.log"),
            &path,
            // Every five minutes, because the host spends faster than that.
            // Under a build this volume loses about 2.7 GB a minute — measured
            // dropping from 53 to 31 GB free in eight minutes — so a quarter of
            // an hour is enough to fall from a healthy 45 GB to the refusal
            // threshold. At fifteen minutes the pass was measured *starting* at
            // 12.5 GB, already refusing work: it arrived at the wreck instead
            // of preventing it. A pass over a volume already inside its budget
            // costs nothing (`freed_bytes=0`, well under a second), so the
            // frequency is only paid for when it is needed.
            "<key>StartInterval</key><integer>300</integer>",
        );
        let health = launchd(
            "com.zvuk.kithara-ci.health",
            &[
                &binary, "ci", "host", "mac", "--config", &config, "--pins", &pins, "health",
            ],
            &logs.join("health.log"),
            &path,
            "<key>StartInterval</key><integer>300</integer>",
        );
        self.write_agent("cleanup", &cleanup)?;
        self.write_agent("health", &health)?;
        // Writing the plist is half the job: launchd keeps the definition it
        // loaded until something replaces it, and this runs as root, outside
        // the GUI domain the agents live in. The cleanup agent on this host
        // ran once an hour for days from an older definition while the file
        // beside it asked for every five minutes.
        info!("maintenance agents staged; load them with `ci host mac activate` as the CI user");
        Ok(())
    }

    /// The daemon runs the copy under `toolchains/shared-bin`, the one the CI
    /// user can replace, for the same reason the maintenance agents do: every
    /// fix to the bridge otherwise needs a password, and the bridge is the part
    /// that gets fixed while it is being brought up. The secrets stay out of
    /// reach — they belong to the sync user, and the CI user cannot read them.
    fn stage_bridge_agent(&self) -> Result<()> {
        let binary = self
            .config
            .host
            .host_root
            .join("toolchains/shared-bin")
            .join(Self::BINARY_NAME)
            .display()
            .to_string();
        let config = self
            .service_root()
            .join("bridge/config.toml")
            .display()
            .to_string();
        let log = self.config.host.host_root.join("logs/bridge.log");
        let plist = bridge_launchd(
            &binary,
            &config,
            &log,
            &self.config.host.agent_path(&self.sync_home()),
            &self.config.host.sync_user,
        );
        let pending = self
            .service_root()
            .join("bridge/com.zvuk.kithara-ci.bridge.plist.pending");
        fs::write(&pending, plist)
            .with_context(|| format!("writing pending bridge service {}", pending.display()))?;
        self.process.run(
            "/usr/sbin/chown",
            &["root:wheel", path_text(&pending)?],
            "set bridge service ownership",
        )
    }

    #[cfg(unix)]
    fn require_private_sync_file(&self, path: &Path) -> Result<()> {
        let metadata = fs::metadata(path).with_context(|| format!("reading {}", path.display()))?;
        if metadata.uid() != self.config.host.sync_uid {
            bail!(
                "{} must be owned by UID {}",
                path.display(),
                self.config.host.sync_uid
            );
        }
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!(
                "{} must not be accessible by group or others",
                path.display()
            );
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn require_private_sync_file(&self, _path: &Path) -> Result<()> {
        bail!("bridge activation requires Unix ownership metadata")
    }

    fn write_agent(&self, name: &str, contents: &str) -> Result<()> {
        let path = self
            .agent_root()
            .join(format!("com.zvuk.kithara-ci.{name}.plist"));
        fs::write(&path, contents)
            .with_context(|| format!("writing launch agent {}", path.display()))?;
        self.process.run(
            "/usr/sbin/chown",
            &[
                &format!("{}:staff", self.config.host.ci_user),
                path_text(&path)?,
            ],
            "set launch agent ownership",
        )?;
        self.process.run(
            "/bin/chmod",
            &["0644", path_text(&path)?],
            "set launch agent permissions",
        )
    }

    fn install_directory(&self, path: &Path, mode: &str, owner: &str) -> Result<()> {
        self.process.run(
            "/usr/bin/install",
            &[
                "-d",
                "-m",
                mode,
                "-o",
                owner,
                "-g",
                if owner == "root" { "wheel" } else { "staff" },
                path_text(path)?,
            ],
            "install service directory",
        )
    }

    fn install_file(&self, source: &Path, target: &Path, mode: &str, owner: &str) -> Result<()> {
        self.process.run(
            "/usr/bin/install",
            &[
                "-m",
                mode,
                "-o",
                owner,
                "-g",
                if owner == "root" { "wheel" } else { "staff" },
                path_text(source)?,
                path_text(target)?,
            ],
            "install service file",
        )
    }

    fn install_empty_file(&self, target: &Path, mode: &str, owner: &str) -> Result<()> {
        self.install_file(Path::new("/dev/null"), target, mode, owner)
    }

    fn service_root(&self) -> PathBuf {
        self.config.host.host_root.join("services")
    }

    delegate::delegate! {
        to self {
            #[expr($.join("bin").join(Self::BINARY_NAME))]
            #[call(service_root)]
            fn installed_binary(&self) -> PathBuf;
            #[expr($.join(Self::HOST_CONFIG_NAME))]
            #[call(service_root)]
            fn installed_config(&self) -> PathBuf;
            #[expr($.join(Self::PINS_NAME))]
            #[call(service_root)]
            fn installed_pins(&self) -> PathBuf;
            #[expr($.join("Library/LaunchAgents"))]
            #[call(ci_home)]
            fn agent_root(&self) -> PathBuf;
        }
    }

    /// The copy the CI user can replace, which is what the periodic agents run.
    ///
    /// `services/bin` is `root:wheel`, so a fix to cleanup or health reaches
    /// the host only through an operator with a password. That is how a broken
    /// cleanup stayed broken: the repository had the fix and the mac kept
    /// running the binary from before it. The bridge daemon already runs from
    /// here for exactly this reason.
    fn replaceable_binary(&self) -> PathBuf {
        self.config
            .host
            .host_root
            .join("toolchains/shared-bin")
            .join(Self::BINARY_NAME)
    }

    fn ci_home(&self) -> PathBuf {
        self.config
            .host
            .host_root
            .join("home")
            .join(&self.config.host.ci_user)
    }

    fn sync_home(&self) -> PathBuf {
        self.config
            .host
            .host_root
            .join("home")
            .join(&self.config.host.sync_user)
    }
}

fn require_macos() -> Result<()> {
    if std::env::consts::OS != "macos" {
        bail!("service installation supports macOS only");
    }
    Ok(())
}

fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("path is not UTF-8: {}", path.display()))
}

pub(super) fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub(super) fn launchd(
    label: &str,
    arguments: &[&str],
    log: &Path,
    path: &str,
    extra: &str,
) -> String {
    let arguments = arguments
        .iter()
        .map(|argument| format!("<string>{}</string>", xml(argument)))
        .collect::<String>();
    let log = xml(&log.display().to_string());
    let path = xml(path);
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\"><dict>\
         <key>EnvironmentVariables</key><dict>\
         <key>PATH</key><string>{path}</string>\
         </dict>\
         <key>Label</key><string>{label}</string>\
         <key>ProgramArguments</key><array>{arguments}</array>\
         <key>RunAtLoad</key><true/>{extra}\
         <key>StandardErrorPath</key><string>{log}</string>\
         <key>StandardOutPath</key><string>{log}</string>\
         </dict></plist>\n"
    )
}

fn bridge_launchd(binary: &str, config: &str, log: &Path, path: &str, user: &str) -> String {
    let extra = format!(
        "<key>StartInterval</key><integer>60</integer><key>UserName</key><string>{}</string>",
        xml(user)
    );
    launchd(
        "com.zvuk.kithara-ci.bridge",
        &[
            "/usr/bin/env",
            binary,
            "ci",
            "bridge",
            "reconcile",
            "--config",
            config,
        ],
        log,
        path,
        &extra,
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    #[cfg(unix)]
    use std::{io::Read as _, os::unix::fs::symlink};

    use clap::Parser;

    use super::*;
    use crate::{
        Cli,
        ci::{config::fixture, process::Recording},
    };

    #[cfg(unix)]
    #[test]
    fn service_layout_installs_cleanup_lock_without_replacing_its_inode() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut config = fixture();
        config.host.host_root = directory.path().to_path_buf();
        let locks = directory.path().join("locks");
        fs::create_dir(&locks)?;
        let owner = fs::metadata(&locks)?;
        let process = Process::recording(
            directory.path(),
            Recording::default()
                .with_reply(
                    &["/usr/bin/id", "-u", &config.host.ci_user],
                    &owner.uid().to_string(),
                )
                .with_reply(
                    &[
                        "/usr/bin/dscl",
                        ".",
                        "-read",
                        "/Groups/staff",
                        "PrimaryGroupID",
                    ],
                    &format!("PrimaryGroupID: {}", owner.gid()),
                ),
        );
        let services = ServiceInstaller::new(&config, &process);

        services.ensure_layout()?;

        let lock = locks.join("cleanup.lock");
        let original = fs::metadata(&lock)?;
        fs::write(&lock, b"held by a cleanup worker")?;
        services.ensure_layout()?;

        let installed = fs::metadata(&lock)?;
        let installed_directory = fs::metadata(&locks)?;
        assert_eq!(
            (installed_directory.dev(), installed_directory.ino()),
            (owner.dev(), owner.ino())
        );
        assert_eq!(
            (installed.dev(), installed.ino()),
            (original.dev(), original.ino())
        );
        assert_eq!(fs::read(&lock)?, b"held by a cleanup worker");
        for (path, mode) in [(&locks, 0o750), (&lock, 0o600)] {
            let installed = fs::metadata(path)?;
            assert_eq!(
                (installed.uid(), installed.gid()),
                (owner.uid(), owner.gid())
            );
            assert_eq!(installed.mode() & 0o7777, mode);
        }
        let recording = process.recorded().unwrap();
        assert!(!recording.steps().iter().any(|step| {
            matches!(
                step.program.as_str(),
                "/usr/bin/install" | "/usr/sbin/chown" | "/bin/chmod"
            ) && step.args.iter().any(|argument| {
                argument == path_text(&locks).unwrap() || argument == path_text(&lock).unwrap()
            })
        }));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_lock_rejects_invalid_owner_records_before_metadata_changes() -> Result<()> {
        for (bad_uid, bad_group) in [
            (Some("not-a-uid"), None),
            (Some("4294967295"), None),
            (None, Some("UniqueID: 20")),
            (None, Some("PrimaryGroupID: 20\nPrimaryGroupID: 21")),
            (None, Some("PrimaryGroupID: 4294967295")),
        ] {
            let directory = tempfile::tempdir()?;
            let mut config = fixture();
            config.host.host_root = directory.path().to_path_buf();
            let locks = directory.path().join("locks");
            fs::create_dir(&locks)?;
            let lock = locks.join("cleanup.lock");
            fs::write(&lock, b"held by a cleanup worker")?;
            fs::set_permissions(&locks, Permissions::from_mode(0o700))?;
            fs::set_permissions(&lock, Permissions::from_mode(0o640))?;
            let owner = fs::metadata(&lock)?;
            let uid = owner.uid().to_string();
            let group = format!("PrimaryGroupID: {}", owner.gid());
            let process = Process::recording(
                directory.path(),
                Recording::default()
                    .with_reply(
                        &["/usr/bin/id", "-u", &config.host.ci_user],
                        bad_uid.unwrap_or(&uid),
                    )
                    .with_reply(
                        &[
                            "/usr/bin/dscl",
                            ".",
                            "-read",
                            "/Groups/staff",
                            "PrimaryGroupID",
                        ],
                        bad_group.unwrap_or(&group),
                    ),
            );
            let services = ServiceInstaller::new(&config, &process);

            assert!(services.install_cleanup_lock().is_err());

            for (path, mode) in [(&locks, 0o700), (&lock, 0o640)] {
                let after = fs::metadata(path)?;
                assert_eq!((after.uid(), after.gid()), (owner.uid(), owner.gid()));
                assert_eq!(after.mode() & 0o7777, mode);
            }
            assert_eq!(fs::read(&lock)?, b"held by a cleanup worker");
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_lock_metadata_stays_on_opened_inodes_after_path_replacement() -> Result<()> {
        for replacement in ["file", "directory", "host"] {
            let directory = tempfile::tempdir()?;
            let external = tempfile::tempdir()?;
            let sentinel = external.path().join("cleanup.lock");
            fs::write(&sentinel, b"outside CI storage")?;
            fs::set_permissions(external.path(), Permissions::from_mode(0o711))?;
            fs::set_permissions(&sentinel, Permissions::from_mode(0o644))?;
            let sentinel_before = fs::metadata(&sentinel)?;
            let external_before = fs::metadata(external.path())?;
            let mut config = fixture();
            config.host.host_root = directory.path().join("host");
            fs::create_dir(&config.host.host_root)?;
            let locks = config.host.host_root.join("locks");
            let path = locks.join("cleanup.lock");
            let process = Process::recording(directory.path(), Recording::default());
            let services = ServiceInstaller::new(&config, &process);
            let (host, opened_directory, file) = services.open_cleanup_lock()?;
            fs::write(&path, b"held by a cleanup worker")?;
            opened_directory.set_permissions(Permissions::from_mode(0o700))?;
            file.set_permissions(Permissions::from_mode(0o640))?;
            let directory_before = opened_directory.metadata()?;
            let file_before = file.metadata()?;
            assert!(file_before.is_file());
            assert_eq!(file_before.nlink(), 1);
            match replacement {
                "file" => {
                    fs::rename(&path, locks.join("retained.lock"))?;
                    symlink(&sentinel, &path)?;
                }
                "directory" => {
                    fs::rename(&locks, config.host.host_root.join("retained-locks"))?;
                    symlink(external.path(), &locks)?;
                }
                "host" => {
                    fs::rename(
                        &config.host.host_root,
                        directory.path().join("retained-host"),
                    )?;
                    symlink(external.path(), &config.host.host_root)?;
                }
                _ => unreachable!(),
            }

            assert!(
                services
                    .set_cleanup_lock_metadata(
                        &host,
                        &opened_directory,
                        &file,
                        (file_before.uid(), file_before.gid()),
                    )
                    .is_err(),
                "{replacement}"
            );

            for (handle, before, mode) in [
                (&opened_directory, &directory_before, 0o750),
                (&file, &file_before, 0o600),
            ] {
                let after = handle.metadata()?;
                assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
                assert_eq!((after.uid(), after.gid()), (before.uid(), before.gid()));
                assert_eq!(after.mode() & 0o7777, mode);
            }
            let mut contents = Vec::new();
            (&file).read_to_end(&mut contents)?;
            assert_eq!(contents, b"held by a cleanup worker");
            assert_eq!(fs::read(&sentinel)?, b"outside CI storage");
            for (path, before) in [
                (sentinel.as_path(), &sentinel_before),
                (external.path(), &external_before),
            ] {
                let after = fs::metadata(path)?;
                assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
                assert_eq!(
                    (after.uid(), after.gid(), after.mode()),
                    (before.uid(), before.gid(), before.mode())
                );
            }
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn service_layout_rejects_cleanup_lock_symlinks() -> Result<()> {
        for link in ["directory", "file", "hard", "nonregular"] {
            let directory = tempfile::tempdir()?;
            let external = tempfile::tempdir()?;
            let sentinel = external.path().join("cleanup.lock");
            fs::write(&sentinel, b"outside CI storage")?;
            let sentinel_before = fs::metadata(&sentinel)?;
            let external_before = fs::metadata(external.path())?;
            let mut config = fixture();
            config.host.host_root = directory.path().to_path_buf();
            let locks = directory.path().join("locks");
            if link == "directory" {
                symlink(external.path(), &locks)?;
            } else {
                fs::create_dir(&locks)?;
                if link == "file" {
                    symlink(&sentinel, locks.join("cleanup.lock"))?;
                } else if link == "hard" {
                    fs::hard_link(&sentinel, locks.join("cleanup.lock"))?;
                } else {
                    fs::create_dir(locks.join("cleanup.lock"))?;
                }
            }
            let locks_before = fs::symlink_metadata(&locks)?;
            let process = Process::recording(directory.path(), Recording::default());
            let services = ServiceInstaller::new(&config, &process);

            assert!(services.ensure_layout().is_err(), "{link}");

            assert_eq!(fs::read(&sentinel)?, b"outside CI storage");
            let locks_after = fs::symlink_metadata(&locks)?;
            assert_eq!(
                (locks_after.uid(), locks_after.gid(), locks_after.mode()),
                (locks_before.uid(), locks_before.gid(), locks_before.mode())
            );
            for (path, before) in [
                (sentinel.as_path(), &sentinel_before),
                (external.path(), &external_before),
            ] {
                let after = fs::metadata(path)?;
                assert_eq!(
                    (after.uid(), after.gid(), after.mode()),
                    (before.uid(), before.gid(), before.mode())
                );
            }
            let recording = process.recorded().unwrap();
            let lock = locks.join("cleanup.lock");
            assert!(!recording.steps().iter().any(|step| {
                matches!(step.program.as_str(), "/usr/sbin/chown" | "/bin/chmod")
                    && step
                        .args
                        .last()
                        .is_some_and(|argument| argument == path_text(&lock).unwrap())
            }));
        }
        Ok(())
    }

    #[test]
    fn periodic_agent_commands_are_accepted_by_the_host_cli() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut config = fixture();
        config.host.host_root = directory.path().to_path_buf();
        let process = Process::recording(directory.path(), Recording::default());
        let services = ServiceInstaller::new(&config, &process);
        fs::create_dir_all(services.agent_root())?;
        services.install_maintenance_agents()?;

        for name in ["cleanup", "health"] {
            let path = services
                .agent_root()
                .join(format!("com.zvuk.kithara-ci.{name}.plist"));
            let plist = plist::Value::from_file(path)?;
            let arguments = plist
                .as_dictionary()
                .and_then(|dictionary| dictionary.get("ProgramArguments"))
                .and_then(plist::Value::as_array)
                .context("agent command arguments")?
                .iter()
                .map(|value| value.as_string().context("string command argument"))
                .collect::<Result<Vec<_>>>()?;
            Cli::try_parse_from(arguments)?;
        }
        Ok(())
    }

    #[test]
    fn generated_agent_escapes_paths() {
        let plist = launchd(
            "test",
            &["/a&b", "ci"],
            Path::new("/tmp/a&b.log"),
            "/opt/homebrew/bin:/usr/bin",
            "",
        );
        assert!(plist.contains("<string>/a&amp;b</string>"));
        assert!(plist.contains("<string>/tmp/a&amp;b.log</string>"));
    }

    /// `launchd` hands agents a minimal PATH, so an agent that shells out to a
    /// Homebrew tool (colima -> limactl) dies unless PATH is spelled out.
    #[test]
    fn generated_agent_declares_a_path() {
        let plist = launchd(
            "test",
            &["/opt/homebrew/bin/colima"],
            Path::new("/tmp/a.log"),
            "/opt/homebrew/bin:/opt/homebrew/sbin:/usr/bin:/bin",
            "",
        );
        assert!(plist.contains("<key>EnvironmentVariables</key>"));
        assert!(plist.contains(
            "<key>PATH</key><string>/opt/homebrew/bin:/opt/homebrew/sbin:/usr/bin:/bin</string>"
        ));
    }

    /// The periodic agents ran `services/bin`, which is `root:wheel`. Every
    /// fix to cleanup then needed an operator with a password, and the one
    /// that did not arrive is why this host refused jobs for a week.
    #[test]
    fn the_periodic_agents_run_the_binary_the_ci_user_can_replace() {
        let mut config = fixture();
        config.host.host_root = PathBuf::from("/Volumes/CI");
        let process = Process::new(Path::new("/Volumes/CI"), BTreeMap::new());
        let services = ServiceInstaller::new(&config, &process);

        assert_eq!(
            services.replaceable_binary(),
            PathBuf::from("/Volumes/CI/toolchains/shared-bin/kithara-ci")
        );
    }

    #[test]
    fn bridge_agent_execs_mutable_binary_through_stable_env() {
        let plist = bridge_launchd(
            "/Volumes/CI/toolchains/shared-bin/kithara-ci",
            "/Volumes/CI/services/bridge/config.toml",
            Path::new("/Volumes/CI/logs/bridge.log"),
            "/opt/homebrew/bin:/usr/bin:/bin",
            "kithara-sync",
        );

        assert!(plist.contains(
            "<key>ProgramArguments</key><array>\
             <string>/usr/bin/env</string>\
             <string>/Volumes/CI/toolchains/shared-bin/kithara-ci</string>\
             <string>ci</string><string>bridge</string><string>reconcile</string>\
             <string>--config</string>\
             <string>/Volumes/CI/services/bridge/config.toml</string></array>"
        ));
        assert!(!plist.contains("<string>/bin/sh</string>"));
        assert!(!plist.contains("<string>-c</string>"));
    }
}
