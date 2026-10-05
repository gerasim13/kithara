use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use sha2::{Digest, Sha256};
use tracing::info;

use super::{
    permissions,
    profile::{ImageRunner, LinuxHost},
};
use crate::{
    ci::{config::CiPins, process::Process},
    consts,
};

/// Install only the native image listener. Its root authority belongs to the
/// owner-authorized image workflow; ordinary runner containers retain their
/// isolation and their currently running jobs.
pub(super) fn install(
    process: &Process,
    host: &LinuxHost,
    pins: &CiPins,
    config: &Path,
) -> Result<()> {
    if !cfg!(target_os = "linux") {
        bail!("the native image runner must be installed on Linux");
    }
    if process.capture("id", &["-u"], "read the installer user")? != "0" {
        bail!("install the native image runner as root");
    }
    process.require_tools(&["docker", "systemctl", "tar"])?;
    let runner = host
        .image_runner
        .as_ref()
        .context("this machine's profile defines no native image runner")?;
    let config = fs::canonicalize(config).context("resolving the image runner profile path")?;
    let root = host.cache_root.join("image-runner");
    fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
    permissions::set_mode(&root, 0o700)?;
    let directory = root.join(&pins.actions_runner_version);
    if !directory.is_dir() {
        let staging = tempfile::tempdir_in(&root).context("staging the native runner")?;
        let archive = staging.path().join("runner.tar.gz");
        let (url, checksum) = runner_archive(pins, std::env::consts::ARCH)?;
        let mut response = Client::builder()
            .https_only(true)
            .build()
            .context("building native runner download client")?
            .get(&url)
            .send()
            .with_context(|| format!("downloading {url}"))?
            .error_for_status()
            .context("downloading the pinned native runner")?;
        write_verified_archive(&mut response, checksum, &archive)?;
        let extracted = staging.path().join("runner");
        fs::create_dir(&extracted).context("creating native runner staging directory")?;
        let mut extract = process.command("tar");
        extract
            .args(["--extract", "--gzip", "--file"])
            .arg(&archive)
            .arg("--directory")
            .arg(&extracted);
        process.run_command(&mut extract, "extract the verified native runner")?;
        fs::rename(&extracted, &directory).context("installing the verified native runner")?;
    }
    if !directory.join("run.sh").is_file() {
        bail!(
            "native runner installation at {} is incomplete",
            directory.display()
        );
    }
    let executable = directory.join("kithara-ci");
    let staged =
        tempfile::NamedTempFile::new_in(&directory).context("staging the host executable")?;
    fs::copy(std::env::current_exe()?, staged.path())
        .context("copying the native runner executable")?;
    permissions::set_mode(staged.path(), consts::EXECUTABLE)?;
    staged
        .persist(&executable)
        .context("installing the native runner executable")?;
    let service = format!("kithara-ci-{}.service", runner.name);
    let path = Path::new(consts::SERVICE_SYSTEMD_ROOT).join(&service);
    fs::write(&path, unit(runner, &directory, &executable, &config)?)
        .with_context(|| format!("writing {}", path.display()))?;
    process.run(
        "systemctl",
        &["daemon-reload"],
        "reload the native image runner unit",
    )?;
    process.run(
        "systemctl",
        &["enable", "--now", &service],
        "enable the native image runner",
    )?;
    info!(runner = runner.name, "native image runner installed");
    Ok(())
}

fn runner_archive<'a>(pins: &'a CiPins, architecture: &str) -> Result<(String, &'a str)> {
    let (slice, checksum) = match architecture {
        "x86_64" => ("x64", pins.actions_runner_linux_amd64_sha256.as_str()),
        "aarch64" => ("arm64", pins.actions_runner_linux_arm64_sha256.as_str()),
        _ => bail!("native Actions runner does not support architecture {architecture}"),
    };
    let version = &pins.actions_runner_version;
    Ok((
        format!(
            "https://github.com/actions/runner/releases/download/v{version}/\
             actions-runner-linux-{slice}-{version}.tar.gz"
        ),
        checksum,
    ))
}

/// Commit no archive to extraction until its complete streamed bytes match
/// the reviewed checksum. The download never needs an archive-sized buffer.
fn write_verified_archive(source: &mut impl Read, expected: &str, path: &Path) -> Result<()> {
    let mut file =
        fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; consts::BUFFER_SIZE];
    loop {
        let read = source
            .read(&mut buffer)
            .context("reading the native runner archive")?;
        if read == 0 {
            break;
        }
        let bytes = &buffer[..read];
        file.write_all(bytes)
            .context("writing the native runner archive")?;
        digest.update(bytes);
    }
    let actual = hex::encode(digest.finalize());
    if actual != expected {
        bail!("native runner SHA-256 mismatch: expected {expected}, found {actual}");
    }
    file.sync_all()
        .context("syncing the verified native runner archive")
}

/// The required pre command creates the initially absent secret file. The
/// `:` command modifier keeps native paths literal, including dollar signs.
fn unit(
    runner: &ImageRunner,
    directory: &Path,
    executable: &Path,
    config: &Path,
) -> Result<String> {
    Ok(format!(
        "[Unit]\n\
         Description=Kithara native image runner\n\
         After=network-online.target docker.service\n\
         Wants=network-online.target\n\
         Requires=docker.service\n\n\
         [Service]\n\
         User=root\n\
         WorkingDirectory={directory}\n\
         Environment=RUNNER_ALLOW_RUNASROOT=1\n\
         Environment=PATH=/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin\n\
         RuntimeDirectory=kithara-ci-image-runner\n\
         RuntimeDirectoryMode=0700\n\
         EnvironmentFile=-{environment}\n\
         ExecStartPre=:{executable} ci host linux --config {config} configure-image-runner --env-file {environment}\n\
         ExecStart=:{entrypoint}\n\
         Restart=always\n\
         RestartSec=5\n\n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        directory = unit_path(directory)?,
        environment = unit_path(&environment_file(runner))?,
        executable = unit_path(executable)?,
        config = unit_path(config)?,
        entrypoint = unit_path(&directory.join("run.sh"))?,
    ))
}

fn environment_file(runner: &ImageRunner) -> PathBuf {
    Path::new("/run/kithara-ci-image-runner").join(format!("{}.env", runner.name))
}

fn unit_path(path: &Path) -> Result<String> {
    let text = path
        .to_str()
        .context("the native runner path is not UTF-8")?;
    if text.contains(['\n', '\r']) {
        bail!("the native runner path cannot contain a line break");
    }
    Ok(format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    ))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::ci::config::fixture;

    #[test]
    fn a_runner_archive_uses_the_checksum_of_its_architecture() {
        let pins = &fixture().pins;
        for (architecture, slice, expected) in [
            ("x86_64", "x64", &pins.actions_runner_linux_amd64_sha256),
            ("aarch64", "arm64", &pins.actions_runner_linux_arm64_sha256),
        ] {
            let (url, checksum) = runner_archive(pins, architecture).expect("supported archive");
            assert!(url.ends_with(&format!(
                "actions-runner-linux-{slice}-{}.tar.gz",
                pins.actions_runner_version
            )));
            assert_eq!(checksum, expected);
        }
        assert!(runner_archive(pins, "riscv64").is_err());
    }

    #[test]
    fn corrupted_runner_bytes_never_pass_the_extraction_boundary() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("runner.tar.gz");
        let checksum = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(write_verified_archive(&mut Cursor::new(b"corrupt"), checksum, &path).is_err());
        write_verified_archive(&mut Cursor::new(b"abc"), checksum, &path).expect("verified bytes");
        assert_eq!(fs::read(path).expect("verified archive"), b"abc");
    }

    #[test]
    fn every_native_listener_start_requires_a_fresh_secret_registration() {
        let runner = ImageRunner {
            name: "images".to_owned(),
            repository: "octocat/kithara".to_owned(),
            labels: vec!["images-host".to_owned()],
        };
        let directory = Path::new("/var/lib/${ROOT}/image-runner/2.336.0");
        let config = Path::new("/etc/${PROFILE}/linux-host.toml");
        let text =
            unit(&runner, directory, &directory.join("kithara-ci"), config).expect("native unit");
        let pre = text
            .lines()
            .find(|line| line.starts_with("ExecStartPre="))
            .expect("required registration");
        assert!(pre.contains("configure-image-runner --env-file"));
        assert!(!pre.contains("ExecStartPre=-"));
        assert!(pre.starts_with("ExecStartPre=:"));
        assert!(pre.contains("--config \"/etc/${PROFILE}/linux-host.toml\""));
        assert!(text.contains("EnvironmentFile=-\"/run/kithara-ci-image-runner/images.env\""));
        assert!(text.contains("RuntimeDirectoryMode=0700"));
        assert!(text.contains("Environment=RUNNER_ALLOW_RUNASROOT=1"));
        assert!(text.contains("Environment=PATH=/root/.cargo/bin:"));
        let start = text
            .lines()
            .find(|line| line.starts_with("ExecStart="))
            .expect("native listener");
        assert_eq!(
            start,
            format!("ExecStart=:\"{}/run.sh\"", directory.display())
        );
        assert!(!start.contains("jitconfig"));
        assert!(text.contains("Restart=always"));
    }

    #[test]
    fn systemd_paths_preserve_spaces_and_literal_specifiers() {
        assert_eq!(
            unit_path(Path::new("/ci % root/runner")).expect("quoted path"),
            "\"/ci %% root/runner\""
        );
        assert!(unit_path(Path::new("/ci\nrunner")).is_err());
        let path = Path::new("/ci ${ROOT}/runner");
        assert_eq!(
            unit_path(path).expect("property path"),
            "\"/ci ${ROOT}/runner\""
        );
    }
}
