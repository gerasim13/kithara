use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use kithara_devtools::lock::FileLock;

use super::{pool::SlotPool, prune, sources, tracked, units};
use crate::config::LaneFreshness;

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
    claim: Claim,
}

/// How the claim made the slot honest for the checkout.
enum Claim {
    Mtime(sources::Claimed),
    Checksum(units::Claimed),
}

impl LaneBuild {
    /// Takes the first free slot of the pool and prunes the units its builds
    /// stopped using, then makes the slot honest for the checkout: an mtime
    /// lane stamps the files the slot may hold artifacts of other content
    /// for, a checksum lane decides every build-script run.
    pub(crate) fn claim(
        project_root: &Path,
        pool: &SlotPool,
        window: Duration,
        freshness: LaneFreshness,
    ) -> Result<Self> {
        let (dir, lock) = pool.take()?;
        fs::create_dir_all(&dir)
            .with_context(|| format!("creating lane build directory {}", dir.display()))?;
        prune::prune(&dir, window)?;
        let tracked = tracked::list(project_root)?;
        let claim = match freshness {
            LaneFreshness::Mtime => Claim::Mtime(sources::claim(project_root, &dir, tracked)?),
            LaneFreshness::Checksum => Claim::Checksum(units::claim(
                project_root,
                &dir,
                tracked,
                SystemTime::now(),
            )?),
        };
        Ok(Self {
            _lock: lock,
            dir,
            claim,
        })
    }

    /// Records what the job's builds left.
    pub(crate) fn settle(&self, succeeded: bool) -> Result<()> {
        match &self.claim {
            Claim::Mtime(claimed) => claimed.settle(succeeded),
            Claim::Checksum(claimed) => claimed.settle(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ci::{
            environment::CacheTrust,
            lane_build::fixture::{git_checkout, set_mtime},
        },
        config::LaneFreshness,
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

        let _claim =
            LaneBuild::claim(checkout.path(), &pool, consts::DAY, LaneFreshness::Mtime).unwrap();

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
