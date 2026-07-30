use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use super::runners::{RunnerManager, path_text, require_macos};

impl RunnerManager<'_> {
    pub(super) fn prepare_guest(&self) -> Result<()> {
        require_macos()?;
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("guest HOME is not set")?;
        let shared = &self.config.cilicon_guest_shared_root;
        for (name, source) in [
            (".cargo", shared.join("kithara-cargo")),
            (".rustup", shared.join("kithara-rustup")),
        ] {
            let target = home.join(name);
            remove_guest_path(&target)?;
            create_guest_symlink(&source, &target)?;
        }
        sudo(
            &[
                "xcode-select",
                "-s",
                path_text(&self.config.cilicon_xcode_developer_dir)?,
            ],
            "select guest Xcode",
        )?;
        sudo(
            &[
                "install",
                "-m",
                "0755",
                path_text(&shared.join("kithara-tools/xcodegen"))?,
                "/usr/local/bin/xcodegen",
            ],
            "install guest xcodegen",
        )?;
        sudo(
            &["install", "-d", "-m", "0755", "/etc/gitlab-runner/certs"],
            "create guest runner CA directory",
        )?;
        let ca_name = format!("{}.crt", self.config.gitlab_host()?);
        sudo(
            &[
                "install",
                "-m",
                "0644",
                path_text(&shared.join("kithara-certs").join(&ca_name))?,
                path_text(&Path::new("/etc/gitlab-runner/certs").join(&ca_name))?,
            ],
            "install guest runner CA",
        )?;
        let xcode =
            self.process
                .capture("/usr/bin/xcodebuild", &["-version"], "guest Xcode version")?;
        if !xcode.starts_with(&format!("Xcode {}\n", self.config.expected_xcode_version)) {
            bail!(
                "guest Xcode does not match {}",
                self.config.expected_xcode_version
            );
        }
        self.process
            .require_tools(&["cargo", "just", "sccache", "xcodegen"])?;
        Ok(())
    }
}

fn remove_guest_path(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path).with_context(|| format!("removing {}", path.display()))
    } else {
        fs::remove_file(path).with_context(|| format!("removing {}", path.display()))
    }
}

#[cfg(unix)]
fn create_guest_symlink(source: &Path, target: &Path) -> Result<()> {
    std::os::unix::fs::symlink(source, target).with_context(|| {
        format!(
            "linking guest {} to shared {}",
            target.display(),
            source.display()
        )
    })
}

#[cfg(not(unix))]
fn create_guest_symlink(_source: &Path, _target: &Path) -> Result<()> {
    bail!("Cilicon guest preparation requires Unix symlinks")
}

fn sudo(args: &[&str], label: &str) -> Result<()> {
    let status = std::process::Command::new("/usr/bin/sudo")
        .arg("-n")
        .args(args)
        .status()
        .with_context(|| format!("starting {label}"))?;
    if !status.success() {
        bail!(
            "{label} failed with exit code {}",
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}
