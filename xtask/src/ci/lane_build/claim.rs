use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use kithara_devtools::lock::FileLock;

use super::{pool::SlotPool, prune, sources, tracked};

/// One job's hold on a lane slot.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get, deref = false)]
pub(crate) struct LaneBuild {
    _lock: FileLock,
    #[field(
        deref = Path,
        get(vis = "pub(crate)", doc = "The directory Cargo builds in.")
    )]
    dir: PathBuf,
    claimed: sources::Claimed,
}

impl LaneBuild {
    /// Takes the first free slot of the pool and prunes the units its builds
    /// stopped using, then stamps the checkout's files the slot may hold
    /// artifacts of other content for.
    pub(crate) fn claim(project_root: &Path, pool: &SlotPool, window: Duration) -> Result<Self> {
        let (dir, lock) = pool.take()?;
        fs::create_dir_all(&dir)
            .with_context(|| format!("creating lane build directory {}", dir.display()))?;
        prune::prune(&dir, window)?;
        let claimed = sources::claim(project_root, &dir, tracked::list(project_root)?)?;
        Ok(Self {
            _lock: lock,
            dir,
            claimed,
        })
    }

    /// Records what the job's builds came from.
    pub(crate) fn settle(&self, succeeded: bool) -> Result<()> {
        self.claimed.settle(succeeded)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, time::SystemTime};

    use super::*;
    use crate::{
        ci::{
            environment::CacheTrust,
            lane_build::fixture::{git_checkout, set_mtime},
        },
        consts,
    };

    #[test]
    fn a_claim_prunes_what_the_lane_stopped_using() {
        let checkout = git_checkout(&[("lib.rs", "one")]);
        let lanes = tempfile::tempdir().unwrap();
        let pool = SlotPool::fleet(lanes.path(), CacheTrust::Review, "test");
        let fingerprints = lanes.path().join("review-lane-test-0/debug/.fingerprint");
        let old = fingerprints.join("old-0123456789abcdef");
        let fresh = fingerprints.join("fresh-fedcba9876543210");
        for unit in [&old, &fresh] {
            fs::create_dir_all(unit).unwrap();
            fs::write(unit.join("lib"), "hash").unwrap();
        }
        set_mtime(&old.join("lib"), SystemTime::now() - 2 * consts::DAY);

        let _claim = LaneBuild::claim(checkout.path(), &pool, consts::DAY).unwrap();

        assert!(
            !old.exists(),
            "a unit the lane stopped using outlived the claim"
        );
        assert!(
            fresh.exists(),
            "the claim removed a unit the lane still uses"
        );
    }
}
