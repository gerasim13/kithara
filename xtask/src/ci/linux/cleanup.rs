use anyhow::Result;
use tracing::{info, warn};

use crate::ci::{config::CiPins, process::Process};

/// Build cache older than this is rebuilt faster than it is worth keeping.
const BUILD_CACHE_AGE: &str = "168h";

/// Reclaim what this project left behind, and nothing else.
///
/// The machine is shared: other stacks keep images and volumes here, so a
/// blanket `docker system prune` would take theirs. Volumes are left alone
/// entirely — `kithara-ci-target` and `kithara-ci-cargo-home` are the caches
/// this exists to protect, and the orphans beside them belong to a setup this
/// code does not own.
pub(super) fn run(process: &Process, pins: &CiPins) -> Result<()> {
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
    Ok(())
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
