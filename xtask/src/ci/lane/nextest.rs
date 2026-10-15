use std::{
    env,
    ffi::OsStr,
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use anyhow::{Context, Result, bail};
use cargo_metadata::semver::Version;
use kithara_devtools::lock::{FileLock, Wait};

use crate::ci::{config::CiPins, environment::CacheTrust, process::Process};

pub(super) fn prepare(process: &Process, pins: &CiPins, cargo: &str) -> Result<Option<PathBuf>> {
    if process.is_recording() {
        return Ok(None);
    }
    let started = Instant::now();
    let version = pins.cargo_tool_version("cargo-nextest")?;
    Version::parse(version).context("nextest pin must be an exact semantic version")?;
    let cache = process
        .cargo_home()
        .context("CI nextest needs the executor's Cargo home")?
        .join("kithara-nextest")
        .join(CacheTrust::read(&|name| process.environment_var(name))?.as_str())
        .join(format!("{}-{}", env::consts::OS, env::consts::ARCH));
    let root = cache.join(version);
    let bin = root.join("bin");
    let executable = bin.join(format!("cargo-nextest{}", env::consts::EXE_SUFFIX));
    fs::create_dir_all(&cache).context("create the pinned nextest cache")?;
    let installed = {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(cache.join(format!("{version}.lock")))?;
        let holder = format!("nextest {version}, pid {}", std::process::id());
        let _lock = FileLock::exclusive(
            file,
            &Wait {
                subject: "pinned nextest installation",
                holder: &holder,
            },
        )?;
        let installed = !present(&executable)?;
        if installed {
            let temp = process
                .environment_path("TMPDIR")
                .unwrap_or_else(env::temp_dir);
            let build =
                tempfile::tempdir_in(temp).context("create temporary nextest build directory")?;
            process.run_command(
                process
                    .command(cargo)
                    .args(["install", "--locked", "--version", version, "--root"])
                    .arg(&root)
                    .arg("cargo-nextest")
                    .current_dir(build.path())
                    .env("CARGO_TARGET_DIR", build.path())
                    .env_remove("RUSTFLAGS")
                    .env_remove("CARGO_ENCODED_RUSTFLAGS")
                    .env_remove("RUSTDOCFLAGS")
                    .env_remove("CARGO_BUILD_TARGET"),
                "install pinned nextest",
            )?;
        }
        let actual = process.transcript(
            process.command(&executable).args(["nextest", "--version"]),
            "read cached nextest version",
        )?;
        require_version(&actual, version)?;
        installed
    };
    process.note_cache(
        "nextest tool",
        if installed { "installed" } else { "reused" },
        format!(
            "{version} at {} in {:.1} s",
            bin.display(),
            started.elapsed().as_secs_f64()
        ),
    );
    Ok(Some(bin))
}

fn present(executable: &Path) -> Result<bool> {
    match fs::symlink_metadata(executable) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => bail!(
            "nextest cache artifact {} is not a regular file",
            executable.display()
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => {
            Err(error).with_context(|| format!("inspect nextest cache {}", executable.display()))
        }
    }
}

fn require_version(actual: &str, version: &str) -> Result<()> {
    let mut words = actual.split_whitespace();
    if words.next() != Some("cargo-nextest") || words.next() != Some(version) {
        bail!("nextest version mismatch: expected {version}, got {actual}");
    }
    Ok(())
}

pub(super) fn prepend(command: &mut Command, bin: &Path) -> Result<()> {
    let path = command
        .get_envs()
        .find_map(|(name, value)| (name == "PATH").then(|| value.map(OsStr::to_os_string)))
        .unwrap_or_else(|| env::var_os("PATH"));
    let mut paths = vec![bin.to_path_buf()];
    if let Some(path) = path {
        paths.extend(env::split_paths(&path));
    }
    command.env(
        "PATH",
        env::join_paths(paths).context("scope the pinned nextest PATH")?,
    );
    Ok(())
}
