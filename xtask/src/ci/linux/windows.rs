use std::{collections::BTreeMap, fs, io::Read, path::Path};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tracing::info;

use super::profile::LinuxHost;
use crate::ci::{config::CiPins, process::Process};

/// What a guest is built from: two tracked files, one vendor download that
/// cannot be pinned, and the tools the lane needs beyond the toolchain. All of
/// it is here rather than pasted into a machine by hand.
struct GuestSources {
    answer_file: &'static str,
    provision_script: &'static str,
    build_tools_url: &'static str,
    cargo_tools: [&'static str; 2],
}

const GUEST: GuestSources = GuestSources {
    answer_file: "ci/windows/autounattend.xml",
    provision_script: "ci/windows/provision.ps1",
    build_tools_url: "https://aka.ms/vs/17/release/vs_buildtools.exe",
    cargo_tools: ["cargo-nextest", "just"],
};

/// Everything the guest's first sign-in needs to know, written beside the
/// answer file so the script carries no versions of its own.
#[derive(Serialize)]
struct GuestSettings<'a> {
    build_tools_url: &'a str,
    /// Empty: Microsoft replaces this bootstrapper in place, so the guest
    /// verifies its signature instead. See ci/windows/provision.ps1.
    build_tools_sha256: &'a str,
    cargo_tools: BTreeMap<&'a str, &'a str>,
    runner_sha256: &'a str,
    runner_url: String,
    rustup_sha256: &'a str,
    rustup_url: String,
    stable_toolchain: &'a str,
}

/// Install the Windows guest that serves the Windows lane.
///
/// Windows Setup reads its answers from any attached volume, so the guest is
/// built by handing it two disks: the installation media, and a small one
/// carrying the answer file, the provisioning script, and the pinned versions
/// that script installs. Nothing is typed into a console.
pub(super) fn install(
    process: &Process,
    host: &LinuxHost,
    pins: &CiPins,
    root: &Path,
) -> Result<()> {
    let guest = host
        .windows
        .as_ref()
        .context("this machine's profile defines no Windows guest")?;
    process.require_tools(&["virt-install", "xorriso"])?;

    let media = host.cache_root.join("iso/windows-eval.iso");
    verify_media(
        &media,
        &pins.windows_eval_iso_sha256,
        &pins.windows_eval_iso_url,
    )?;

    let answers = build_answer_media(process, host, pins, root)?;

    let mut command = process.command("virt-install");
    command.args([
        "--name",
        &guest.name,
        "--osinfo",
        "win11",
        "--boot",
        "uefi",
        "--tpm",
        "backend.type=emulator,backend.version=2.0,model=tpm-crb",
        "--vcpus",
        &guest.vcpus.to_string(),
        "--memory",
        &guest.memory_mib.to_string(),
        "--disk",
        &format!("size={},format=qcow2", guest.disk_gib),
        "--disk",
        &format!("{},device=cdrom", path_text(&media)?),
        "--disk",
        &format!("{},device=cdrom", path_text(&answers)?),
        "--network",
        &format!("network={}", host.network),
        "--graphics",
        "none",
        "--noautoconsole",
        "--wait",
        "-1",
    ]);
    process.run_command(&mut command, "install the Windows guest")?;
    info!(guest = guest.name, "Windows guest installed");
    Ok(())
}

/// Build the small disk Windows Setup reads its answers from.
fn build_answer_media(
    process: &Process,
    host: &LinuxHost,
    pins: &CiPins,
    root: &Path,
) -> Result<std::path::PathBuf> {
    let staging = host.cache_root.join("windows-answers");
    if staging.exists() {
        fs::remove_dir_all(&staging).with_context(|| format!("clearing {}", staging.display()))?;
    }
    fs::create_dir_all(&staging).with_context(|| format!("creating {}", staging.display()))?;

    for source in [GUEST.answer_file, GUEST.provision_script] {
        let name = Path::new(source)
            .file_name()
            .context("a tracked file with no name")?;
        fs::copy(root.join(source), staging.join(name))
            .with_context(|| format!("copying {source}"))?;
    }

    let settings = GuestSettings {
        build_tools_url: GUEST.build_tools_url,
        build_tools_sha256: "",
        cargo_tools: GUEST
            .cargo_tools
            .iter()
            .map(|tool| Ok((*tool, pins.cargo_tool_version(tool)?)))
            .collect::<Result<_>>()?,
        runner_sha256: &pins.actions_runner_windows_sha256,
        runner_url: format!(
            "https://github.com/actions/runner/releases/download/v{version}/actions-runner-win-x64-{version}.zip",
            version = pins.actions_runner_version,
        ),
        rustup_sha256: &pins.rustup_windows_sha256,
        rustup_url: format!(
            "https://static.rust-lang.org/rustup/archive/{}/x86_64-pc-windows-msvc/rustup-init.exe",
            pins.rustup_version,
        ),
        stable_toolchain: &pins.stable_toolchain,
    };
    fs::write(
        staging.join("guest.json"),
        serde_json::to_vec_pretty(&settings).context("serialising the guest settings")?,
    )
    .context("writing the guest settings")?;

    let media = host.cache_root.join("windows-answers.iso");
    let mut command = process.command("xorriso");
    command.args([
        "-as",
        "mkisofs",
        "-quiet",
        "-J",
        "-rock",
        "-volid",
        "ANSWERS",
        "-output",
        path_text(&media)?,
        path_text(&staging)?,
    ]);
    process.run_command(&mut command, "build the answer media")?;
    Ok(media)
}

/// Refuse to install from media the pins do not vouch for. A guest built from
/// a substituted image would look identical from the outside.
fn verify_media(path: &Path, expected: &str, source: &str) -> Result<()> {
    if !path.is_file() {
        bail!(
            "the Windows installation media is missing from {}; download it from {source}",
            path.display()
        );
    }
    let mut file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("hashing {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = hex::encode(hasher.finalize());
    if actual != expected {
        bail!(
            "the Windows installation media at {} hashes to {actual}, not the pinned {expected}",
            path.display()
        );
    }
    Ok(())
}

fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("path is not UTF-8: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ci::config::fixture;

    #[test]
    fn the_guest_is_told_versions_rather_than_left_to_choose() {
        let pins = &fixture().pins;
        let settings = GuestSettings {
            build_tools_url: GUEST.build_tools_url,
            build_tools_sha256: "",
            cargo_tools: GUEST
                .cargo_tools
                .iter()
                .map(|tool| (*tool, pins.cargo_tool_version(tool).unwrap()))
                .collect(),
            runner_sha256: &pins.actions_runner_windows_sha256,
            runner_url: String::new(),
            rustup_sha256: &pins.rustup_windows_sha256,
            rustup_url: String::new(),
            stable_toolchain: &pins.stable_toolchain,
        };
        let json = serde_json::to_string(&settings).unwrap();

        assert!(json.contains(pins.stable_toolchain.as_str()), "{json}");
        assert!(
            json.contains(pins.cargo_tool_version("cargo-nextest").unwrap()),
            "{json}"
        );
    }
}
