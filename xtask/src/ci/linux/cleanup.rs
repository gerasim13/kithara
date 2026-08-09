use std::path::PathBuf;

use anyhow::{Result, bail};
use tracing::{info, warn};

use super::profile::LinuxHost;
use crate::ci::{build_cache, config::CiPins, process::Process};

/// Build cache older than this is rebuilt faster than it is worth keeping.
const BUILD_CACHE_AGE: &str = "168h";

/// Reclaim what this project left behind, and nothing else.
///
/// The machine is shared: other stacks keep images and volumes here, so a
/// blanket `docker system prune` would take theirs. Project volumes stay in
/// place: target contents are budgeted here, Cargo home remains protected, and
/// orphan volumes belong to a setup this code does not own.
pub(super) fn run(process: &Process, host: &LinuxHost, pins: &CiPins) -> Result<()> {
    let Some((_, pinned)) = pins.linux_image.rsplit_once(':') else {
        warn!(image = pins.linux_image, "pinned image carries no tag");
        return Ok(());
    };

    let listed = process.capture(
        "docker",
        &[
            "images",
            "kithara-ci*",
            "--format",
            "{{.Repository}}:{{.Tag}}",
        ],
        "list project images",
    )?;
    let superseded: Vec<&str> = listed
        .lines()
        .map(str::trim)
        .filter(|image| !image.is_empty())
        .filter(|image| image.rsplit_once(':').is_none_or(|(_, tag)| tag != pinned))
        .collect();

    for image in &superseded {
        process.best_effort("docker", &["rmi", image], "remove a superseded image");
    }
    info!(
        removed = superseded.len(),
        kept = pinned,
        "superseded project images removed"
    );

    process.best_effort(
        "docker",
        &[
            "builder",
            "prune",
            "--force",
            "--filter",
            &format!("until={BUILD_CACHE_AGE}"),
        ],
        "prune the build cache",
    );
    let target_dirs = target_dirs(process)?;
    build_cache::enforce_budget(&target_dirs, host.build_cache_budget_bytes()?)?;
    Ok(())
}

fn target_dirs(process: &Process) -> Result<Vec<PathBuf>> {
    const VOLUME_PREFIX: &str = "kithara-ci-target";

    let filter = format!("name={VOLUME_PREFIX}");
    let listed = process.capture(
        "docker",
        &["volume", "ls", "--filter", &filter, "--format", "{{.Name}}"],
        "list project target volumes",
    )?;
    let mut names: Vec<&str> = listed
        .lines()
        .map(str::trim)
        .filter(|name| name.starts_with(VOLUME_PREFIX))
        .collect();
    names.sort_unstable();

    let mut target_dirs = Vec::with_capacity(names.len());
    for name in names {
        let label = format!("inspect project target volume {name}");
        let mountpoint = process.capture(
            "docker",
            &["volume", "inspect", "--format", "{{.Mountpoint}}", name],
            &label,
        )?;
        let path = PathBuf::from(mountpoint);
        if !path.is_absolute() {
            bail!("Docker target volume {name} returned a non-absolute mountpoint");
        }
        target_dirs.push(path);
    }
    Ok(target_dirs)
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_tags_other_than_the_pinned_one_are_superseded() {
        let pinned = "linux-20260806d";
        let listed = "kithara-ci:linux-20260729\n\
                      kithara-ci:linux-20260806d\n\
                      kithara-ci-android:linux-20260806c\n\
                      kithara-ci-runner:linux-20260806d\n";
        let superseded: Vec<&str> = listed
            .lines()
            .map(str::trim)
            .filter(|image| !image.is_empty())
            .filter(|image| image.rsplit_once(':').is_none_or(|(_, tag)| tag != pinned))
            .collect();
        assert_eq!(
            superseded,
            [
                "kithara-ci:linux-20260729",
                "kithara-ci-android:linux-20260806c"
            ]
        );
    }
}
