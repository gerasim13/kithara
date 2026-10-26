#![cfg(target_os = "macos")]

use std::{
    fs,
    io::{Cursor, Write as _},
    os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use kithara_devtools::{lease, lock::FileLock};
use plist::Value;
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

const POLICY: &str = r#"(version 1)(allow default)
(deny file-write* (subpath "/tmp/kithara-ci") (subpath "/private/tmp/kithara-ci")
 (subpath "/tmp/.kithara-job-leases") (subpath "/private/tmp/.kithara-job-leases")
 (subpath (param "TRIPWIRE")))"#;

struct Fixture {
    backing: Option<TempDir>,
    image: PathBuf,
    root: PathBuf,
    device: Option<String>,
    child: Option<Child>,
    retain: bool,
}

fn entities(value: &Value) -> Result<&[Value]> {
    value
        .as_dictionary()
        .and_then(|v| v.get("system-entities"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .context("owned image entities")
}

fn field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.as_dictionary()?.get(key)?.as_string()
}

impl Fixture {
    fn new() -> Result<Self> {
        let backing = tempfile::tempdir()?;
        let directory = backing.path().canonicalize()?;
        let root = Path::new("/Volumes").join(format!(
            "kithara-admission-{}",
            directory
                .file_name()
                .context("fixture name")?
                .to_string_lossy()
        ));
        ensure!(
            matches!(fs::symlink_metadata(&root), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
            "fixture mountpoint is not absent"
        );
        let mut fixture = Self {
            image: directory.join("fixture.dmg"),
            backing: Some(backing),
            root,
            device: None,
            child: None,
            retain: false,
        };
        let created = fixture.run(
            Command::new("/usr/bin/hdiutil")
                .args([
                    "create", "-size", "64m", "-fs", "HFS+", "-type", "UDIF", "-volname",
                ])
                .arg(fixture.root.file_name().context("volume name")?)
                .arg(&fixture.image),
            30,
            false,
        )?;
        ensure!(
            created.status.success(),
            "create disposable HFS+ image: {}",
            String::from_utf8_lossy(&created.stderr)
        );
        fixture.image = fixture.image.canonicalize()?;
        ensure!(
            fixture.image.parent() == Some(directory.as_path()),
            "image escaped its temporary directory"
        );
        let device = fs::metadata(&fixture.image)?.dev();
        let temporary_device = fs::metadata(&directory)?.dev();
        ensure!(
            device == temporary_device,
            "backing image changed filesystem"
        );
        let receipt = format!(
            "backing={} device={device} temporary_device={temporary_device} mount={}\n",
            fixture.image.display(),
            fixture.root.display()
        );
        fs::write(directory.join("receipt"), &receipt)?;
        eprintln!("{receipt}");
        fixture.retain = true;
        let attached = fixture.run(
            Command::new("/usr/bin/hdiutil")
                .args(["attach", "-plist", "-nobrowse", "-mountpoint"])
                .arg(&fixture.root)
                .arg(&fixture.image),
            30,
            false,
        )?;
        fs::write(directory.join("attachment.plist"), &attached.stdout)?;
        ensure!(
            attached.status.success(),
            "mount prerequisite failed: {}",
            String::from_utf8_lossy(&attached.stderr)
        );
        let value = Value::from_reader(Cursor::new(&attached.stdout))?;
        let rows = entities(&value)?;
        ensure!(
            rows.iter()
                .any(|v| field(v, "mount-point") == fixture.root.to_str()),
            "unexpected image mount"
        );
        let devices: Vec<_> = rows
            .iter()
            .filter_map(|v| field(v, "dev-entry"))
            .filter(|v| {
                v.strip_prefix("/dev/disk")
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            })
            .collect();
        ensure!(
            devices.len() == 1,
            "attachment did not identify exactly one whole device"
        );
        fixture.device = Some(devices[0].to_owned());
        ensure!(
            fixture.root.canonicalize()? == fixture.root
                && fs::symlink_metadata(&fixture.root)?.is_dir(),
            "mountpoint identity is not canonical"
        );
        fixture.retain = false;
        for name in [
            "cache",
            "logs",
            "toolchains",
            "vm/tart/vms/base",
            "workspaces",
            "locks",
            "bin",
            "brew/bin",
            "tmp",
            "home",
            "android",
            "shared",
            "Xcode.app/Contents/Developer",
        ] {
            fs::create_dir_all(fixture.root.join(name))?;
        }
        fs::write(fixture.root.join("locks/cleanup.lock"), "")?;
        for role in ["git", "kache", "sccache"] {
            symlink(
                env!("CARGO_BIN_EXE_fake-tool"),
                fixture.root.join("bin").join(role),
            )?;
        }
        let colima = fixture.root.join("brew/bin/colima");
        fs::write(
            &colima,
            r#"#!/bin/sh
set -eu
printf 'trim\n' >> "$KITHARA_TEST_TRACE"
case "$KITHARA_TEST_SCENARIO" in
 install-kache) /bin/ln -s "$KITHARA_TEST_TOOL" "$KITHARA_TEST_BIN/kache";;
 failure) printf 'private-tool-token\n'; printf 'private-tool-token\n' >&2; exit 7;;
 block) printf '%s\n' "$$" >> "$KITHARA_TEST_PIDS"; exec /bin/sleep 30;;
esac
"#,
        )?;
        fs::set_permissions(colima, fs::Permissions::from_mode(0o755))?;
        fixture.profile(false, 60)?;
        Ok(fixture)
    }

    fn profile(&self, reject: bool, deadline: i64) -> Result<()> {
        let mut profile: toml::Table = include_str!("fixtures/ci-mac-host.toml").parse()?;
        for (key, relative) in [
            ("host_root", ""),
            ("brew_root", "brew"),
            ("android_home", "android"),
            ("cache_root_macos", "cache"),
            ("cache_root_linux", "cache"),
            ("cache_root_windows", "cache"),
            ("macos_guest_shared_root", "shared"),
            ("macos_vm_bundle", "vm/tart/vms/base"),
            ("host_xcode_developer_dir", "Xcode.app/Contents/Developer"),
            (
                "macos_guest_xcode_developer_dir",
                "Xcode.app/Contents/Developer",
            ),
        ] {
            profile.insert(
                key.into(),
                self.root
                    .join(relative)
                    .to_string_lossy()
                    .into_owned()
                    .into(),
            );
        }
        for (key, value) in [
            ("soft_cleanup_bytes", 1),
            ("aggressive_cleanup_bytes", 2),
            ("reject_bytes", 3),
            ("quota_bytes", 4),
            ("job_admission_deadline_seconds", deadline),
            ("cleanup_deadline_seconds", 60),
            ("log_limit_bytes", 1),
        ] {
            profile.insert(key.into(), value.into());
        }
        let source = toml::to_string(&profile)?;
        let source = if reject {
            source.replace("quota_bytes = 4", "quota_bytes = 9223372036854775807")
        } else {
            source
        };
        fs::write(self.root.join("profile.toml"), source)?;
        Ok(())
    }

    fn run(&mut self, command: &mut Command, seconds: u64, sandboxed: bool) -> Result<Output> {
        let directory = if sandboxed {
            self.root.clone()
        } else {
            self.image
                .parent()
                .context("image directory")?
                .to_path_buf()
        };
        let stdout = directory.join("stdout");
        let stderr = directory.join("stderr");
        command
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout)?)
            .stderr(fs::File::create(&stderr)?);
        self.child = Some(command.spawn().with_context(|| {
            format!(
                "start fixture command {}",
                command.get_program().to_string_lossy()
            )
        })?);
        let deadline = Instant::now() + Duration::from_secs(seconds);
        let status = loop {
            if let Some(status) = self.child.as_mut().context("owned command")?.try_wait()? {
                break status;
            }
            ensure!(
                Instant::now() < deadline,
                "owned fixture command exceeded its deadline"
            );
            thread::sleep(Duration::from_millis(20));
        };
        self.child.take();
        Ok(Output {
            status,
            stdout: fs::read(&stdout)
                .with_context(|| format!("read fixture stdout {}", stdout.display()))?,
            stderr: fs::read(&stderr)
                .with_context(|| format!("read fixture stderr {}", stderr.display()))?,
        })
    }

    fn cli(&mut self, operation: &str, scenario: &str) -> Result<(bool, String)> {
        ensure!(
            matches!(operation, "preflight" | "cleanup"),
            "unsupported fixture operation"
        );
        let denied = self.root.join(if scenario == "unwritable" {
            "vm"
        } else {
            "tripwire"
        });
        let mut command = Command::new("/usr/bin/sandbox-exec");
        command
            .arg("-D")
            .arg(format!("TRIPWIRE={}", denied.display()))
            .args(["-p", POLICY])
            .arg(env!("CARGO_BIN_EXE_xtask"))
            .args(["ci", "host", "mac", "--config"])
            .arg(self.root.join("profile.toml"))
            .arg("--pins")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.config/ci-pins.toml"))
            .arg(operation)
            .current_dir(&self.root)
            .env_clear()
            .env("NO_COLOR", "1")
            .env("HOME", self.root.join("home"))
            .env("TMPDIR", self.root.join("tmp"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .env("KITHARA_TEST_TRACE", self.root.join("trace"))
            .env("KITHARA_TEST_PIDS", self.root.join("pids"))
            .env("KITHARA_TEST_BIN", self.root.join("bin"))
            .env("KITHARA_TEST_TOOL", env!("CARGO_BIN_EXE_fake-tool"))
            .env("KITHARA_TEST_SCENARIO", scenario);
        let retained = self.retain;
        self.retain = true;
        let output = self.run(
            &mut command,
            if scenario == "block" { 20 } else { 65 },
            true,
        )?;
        let stderr = String::from_utf8(output.stderr)?;
        ensure!(
            !stderr.contains("private-tool-token")
                && !output
                    .stdout
                    .windows(b"private-tool-token".len())
                    .any(|v| v == b"private-tool-token"),
            "untrusted tool output escaped capture"
        );
        let confirmed = if operation == "cleanup" {
            (output.status.success()
                && stderr.contains("host maintenance finished outcome=Completed"))
                || (!output.status.success()
                    && stderr.contains("host maintenance finished outcome=Failed"))
        } else {
            (output.status.success() && stderr.contains("host preflight passed"))
                || stderr.contains("required host checks did not complete: Failed")
                || stderr.contains("required host checks did not complete: TimedOut")
        };
        self.retain = retained || !confirmed;
        Ok((output.status.success(), stderr))
    }

    fn detach(&mut self) -> Result<()> {
        self.stop_child();
        ensure!(
            !self.retain,
            "fixture lifecycle is uncertain; image retained"
        );
        let Some(device) = self.device.clone() else {
            return Ok(());
        };
        let result = (|| -> Result<()> {
            let info = self.run(
                Command::new("/usr/bin/hdiutil").args(["info", "-plist"]),
                10,
                false,
            )?;
            fs::write(self.image.with_extension("pre-detach.plist"), &info.stdout)?;
            ensure!(info.status.success(), "inspect owned image before detach");
            let value = Value::from_reader(Cursor::new(info.stdout))?;
            let images = value
                .as_dictionary()
                .and_then(|v| v.get("images"))
                .and_then(Value::as_array)
                .context("attached image list")?;
            let image = images
                .iter()
                .find(|v| field(v, "image-path") == self.image.to_str())
                .context("own image no longer identifiable")?;
            let rows = entities(image)?;
            ensure!(
                rows.iter()
                    .any(|v| field(v, "dev-entry") == Some(device.as_str()))
                    && rows
                        .iter()
                        .any(|v| field(v, "mount-point") == self.root.to_str()),
                "owned device identity changed"
            );
            let output = self.run(
                Command::new("/usr/bin/hdiutil").args(["detach", &device]),
                10,
                false,
            )?;
            ensure!(
                output.status.success(),
                "normal detach did not confirm removal"
            );
            Ok(())
        })();
        if result.is_ok() {
            self.device = None;
        } else {
            self.retain = true;
        }
        self.stop_child();
        result
    }

    fn stop_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            self.retain = true;
            eprintln!("stopping owned fixture child {}", child.id());
            let _ = child.kill();
            let deadline = Instant::now() + Duration::from_secs(2);
            while !matches!(child.try_wait(), Ok(Some(_))) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
            }
            if !matches!(child.try_wait(), Ok(Some(_))) {
                let _ = fs::write(
                    self.image.with_extension("unconfirmed-child"),
                    format!("pid={} stop/reap unconfirmed\n", child.id()),
                );
                eprintln!("owned fixture child {} stop/reap unconfirmed", child.id());
            }
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop_child();
        if !self.retain
            && let Err(error) = self.detach()
        {
            self.retain = true;
            eprintln!("retaining fixture: {error}");
        }
        self.stop_child();
        if self.retain
            && let Some(backing) = self.backing.take()
        {
            eprintln!("fixture retained at {}", backing.keep().display());
        }
    }
}

#[test]
fn sandboxed_cli_maintenance_precedes_fresh_checks_and_safe_warnings_do_not_admit_host_failures()
-> Result<()> {
    let mut fixture = Fixture::new().context("prepare isolated admission fixture")?;
    let tripwire = fixture.root.join("tripwire");
    fs::create_dir(&tripwire)?;
    let allowed = fixture.root.join("allowed-write");
    let control = fixture.run(
        Command::new("/usr/bin/sandbox-exec")
            .arg("-D")
            .arg(format!("TRIPWIRE={}", tripwire.display()))
            .args(["-p", POLICY, "/usr/bin/touch"])
            .arg(&allowed),
        10,
        true,
    )?;
    ensure!(
        control.status.success() && allowed.exists(),
        "sandbox allowed-write prerequisite failed"
    );
    let denied = fixture.run(
        Command::new("/usr/bin/sandbox-exec")
            .arg("-D")
            .arg(format!("TRIPWIRE={}", tripwire.display()))
            .args(["-p", POLICY, "/usr/bin/touch"])
            .arg(tripwire.join("blocked")),
        10,
        true,
    )?;
    ensure!(
        !denied.status.success() && !tripwire.join("blocked").exists(),
        "sandbox write denial prerequisite failed"
    );
    let sentinel = fixture.root.join("protected-sentinel");
    fs::write(&sentinel, b"external fixture data must survive maintenance")?;
    let hash = Sha256::digest(fs::read(&sentinel)?);
    for pass in 1..=2 {
        fs::remove_file(fixture.root.join("bin/kache"))
            .context("remove kache before ordered maintenance")?;
        let (passed, stderr) = fixture.cli("preflight", "install-kache")?;
        ensure!(
            passed
                && stderr.contains("admission maintenance finished outcome=Completed")
                && stderr.contains("host preflight passed"),
            "{stderr}"
        );
        assert_eq!(
            fs::read_to_string(fixture.root.join("trace"))
                .with_context(|| format!("read maintenance trace after admission: {stderr}"))?
                .lines()
                .count(),
            pass
        );
        assert_eq!(Sha256::digest(fs::read(&sentinel)?), hash);
    }
    optional_maintenance_failures_and_required_host_refusals(&mut fixture, &sentinel, &hash)?;
    physical_recovery_reclaims_idle_native_artifact(&mut fixture, &sentinel, &hash)?;
    short_deadlines_and_blocked_children_remain_bounded(&mut fixture, &sentinel, &hash)?;
    fixture.detach()?;
    Ok(())
}

fn optional_maintenance_failures_and_required_host_refusals(
    fixture: &mut Fixture,
    sentinel: &Path,
    hash: &[u8],
) -> Result<()> {
    let (passed, stderr) = fixture.cli("preflight", "failure")?;
    ensure!(
        passed
            && stderr.contains("could not trim the Linux guest's disk")
            && stderr.contains("host preflight passed"),
        "{stderr}"
    );
    let log = fixture.root.join("logs/external.log");
    fs::hard_link(sentinel, &log)?;
    let (passed, stderr) = fixture.cli("preflight", "normal")?;
    ensure!(
        passed
            && stderr.contains("optional admission maintenance did not complete outcome=Failed")
            && stderr.contains("host preflight passed"),
        "{stderr}"
    );
    assert_eq!(&Sha256::digest(fs::read(sentinel)?)[..], hash);
    let (passed, stderr) = fixture.cli("preflight", "unwritable")?;
    ensure!(
        !passed
            && stderr.contains("optional admission maintenance did not complete outcome=Failed")
            && stderr.contains(&format!(
                "CI directory is not writable: {}",
                fixture.root.join("vm").display()
            ))
            && stderr.contains("required host checks did not complete: Failed")
            && !stderr.contains("host preflight passed"),
        "{stderr}"
    );
    assert_eq!(&Sha256::digest(fs::read(sentinel)?)[..], hash);
    let (passed, stderr) = fixture.cli("cleanup", "normal")?;
    ensure!(
        !passed
            && stderr.contains("host maintenance finished outcome=Failed")
            && stderr.contains("host maintenance did not complete: Failed")
            && !stderr.contains("host preflight passed"),
        "{stderr}"
    );
    assert_eq!(&Sha256::digest(fs::read(sentinel)?)[..], hash);
    fs::remove_file(fixture.root.join("bin/kache"))
        .context("remove kache before required-tool refusal")?;
    let (passed, stderr) = fixture.cli("preflight", "normal")?;
    ensure!(
        !passed
            && stderr.contains("outcome=Failed")
            && stderr.contains("required CI command is missing: kache"),
        "{stderr}"
    );
    assert_eq!(&Sha256::digest(fs::read(sentinel)?)[..], hash);
    fs::remove_file(log)?;
    symlink(
        env!("CARGO_BIN_EXE_fake-tool"),
        fixture.root.join("bin/kache"),
    )?;
    fixture.profile(true, 60)?;
    let (passed, stderr) = fixture.cli("preflight", "normal")?;
    ensure!(
        !passed
            && stderr.contains("is full; new jobs stop here")
            && !stderr.contains("host preflight passed"),
        "{stderr}"
    );
    Ok(())
}

fn physical_recovery_reclaims_idle_native_artifact(
    fixture: &mut Fixture,
    sentinel: &Path,
    hash: &[u8],
) -> Result<()> {
    let mib = 1024 * 1024_u64;
    let soft_floor = 40 * mib;
    let reject_floor = 15 * mib;
    let backing = fixture
        .backing
        .as_ref()
        .context("owned backing directory")?
        .path()
        .canonicalize()?;
    let image_device = fs::symlink_metadata(&fixture.root)?.dev();
    let backing_device = fs::metadata(&backing)?.dev();
    let total = fs4::total_space(&fixture.root)?;
    let initial_free = fs4::available_space(&fixture.root)?;
    ensure!(
        fixture.root.canonicalize()? == fixture.root
            && fixture.image.parent() == Some(backing.as_path())
            && fs::metadata(&fixture.image)?.dev() == backing_device
            && image_device != backing_device
            && total <= 64 * mib
            && initial_free > soft_floor + 4 * mib
            && initial_free <= total,
        "physical recovery requires the bounded owned image on a distinct device"
    );
    fixture.profile(false, 60)?;
    let profile_path = fixture.root.join("profile.toml");
    let mut profile: toml::Table = fs::read_to_string(&profile_path)?.parse()?;
    for (key, bytes) in [
        ("quota_bytes", total),
        ("soft_cleanup_bytes", total - soft_floor),
        ("aggressive_cleanup_bytes", total - 25 * mib),
        ("reject_bytes", total - reject_floor),
    ] {
        profile.insert(key.into(), i64::try_from(bytes)?.into());
    }
    fs::write(&profile_path, toml::to_string(&profile)?)?;
    let workspaces = fixture.root.join("workspaces");
    ensure!(
        workspaces.canonicalize()? == workspaces
            && fs::symlink_metadata(&workspaces)?.is_dir()
            && fs::metadata(&workspaces)?.dev() == image_device,
        "native build scope must stay on the owned image"
    );
    let container_key = Path::new("gitlab/fixture/0/disrupt/kithara.target");
    let container_owner = lease::hold_in(&workspaces, container_key)?;
    let slot = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(workspaces.join(container_key).join("lint-0.lock"))?;
    let slot_owner = FileLock::try_exclusive(slot)?;
    let key = container_key.join("lint-0");
    let owner = lease::hold_in(&workspaces, &key)?;
    let target = workspaces.join(&key);
    ensure!(
        target.canonicalize()? == target && fs::metadata(&target)?.dev() == image_device,
        "native build target escaped the owned image"
    );
    symlink(
        "lint-0",
        target.parent().context("native build root")?.join("build"),
    )?;
    let artifact = target.join("artifact.bin");
    let mut writer = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&artifact)?;
    let block = vec![0xa5_u8; 1024 * 1024];
    let mut written = 0_u64;
    for _ in 0..64 {
        if fs4::available_space(&fixture.root)? < 13 * mib {
            break;
        }
        writer.write_all(&block)?;
        written += mib;
        ensure!(
            fs4::available_space(&fixture.root)? >= 10 * mib,
            "physical recovery must retain at least 10 MiB free"
        );
    }
    writer.sync_all()?;
    let allocated = writer.metadata()?;
    ensure!(
        written > 0
            && written <= 64 * mib
            && allocated.len() == written
            && allocated.blocks().saturating_mul(512) >= written,
        "physical recovery needs bounded allocated bytes, not a sparse file"
    );
    drop(writer);
    drop(owner);
    drop(slot_owner);
    drop(container_owner);
    let before = fs4::available_space(&fixture.root)?;
    eprintln!(
        "physical recovery before total_bytes={total} free_bytes={before} initial_free_bytes={initial_free} written_bytes={written} image_device={image_device} backing_device={backing_device}"
    );
    ensure!(
        before >= 10 * mib && before < reject_floor,
        "physical recovery must start below the rejection floor with safe headroom"
    );
    let (passed, stderr) = fixture.cli("preflight", "normal")?;
    let after_total = fs4::total_space(&fixture.root)?;
    let after = fs4::available_space(&fixture.root)?;
    eprintln!(
        "physical recovery after total_bytes={after_total} free_bytes={after} reclaimed_bytes={}",
        after.saturating_sub(before)
    );
    ensure!(
        passed
            && stderr.contains("admission maintenance finished outcome=Completed")
            && stderr.contains("host preflight passed"),
        "{stderr}"
    );
    ensure!(
        after_total == total
            && fs::metadata(&fixture.root)?.dev() == image_device
            && after > soft_floor,
        "physical recovery did not restore the owned image above its soft floor"
    );
    ensure!(
        matches!(fs::symlink_metadata(&artifact), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
        "physical recovery did not reclaim the idle native artifact"
    );
    assert_eq!(&Sha256::digest(fs::read(sentinel)?)[..], hash);
    Ok(())
}

fn short_deadlines_and_blocked_children_remain_bounded(
    fixture: &mut Fixture,
    sentinel: &Path,
    hash: &[u8],
) -> Result<()> {
    fixture.profile(false, 2)?;
    let started = Instant::now();
    let (passed, stderr) = fixture.cli("preflight", "normal")?;
    ensure!(
        passed && stderr.contains("host preflight passed"),
        "{stderr}"
    );
    ensure!(
        started.elapsed() < Duration::from_secs(4),
        "short admission exceeded its bound"
    );
    fixture.profile(false, 60)?;
    for pass in 1..=2 {
        let started = Instant::now();
        let (passed, stderr) = fixture.cli("preflight", "block")?;
        ensure!(
            passed && stderr.contains("host preflight passed"),
            "{stderr}"
        );
        ensure!(
            stderr.contains("could not trim the Linux guest's disk"),
            "{stderr}"
        );
        ensure!(
            started.elapsed() < Duration::from_secs(15),
            "blocked control command exceeded its bound"
        );
        let pids = fs::read_to_string(fixture.root.join("pids"))
            .with_context(|| format!("read blocked-command PIDs after admission: {stderr}"))?
            .lines()
            .map(str::parse::<i32>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert_eq!(
            pids.len(),
            pass,
            "each admission must start one new blocked command"
        );
        for pid in pids {
            assert_eq!(
                nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
                Err(nix::errno::Errno::ESRCH)
            );
        }
        assert_eq!(&Sha256::digest(fs::read(sentinel)?)[..], hash);
    }
    Ok(())
}
