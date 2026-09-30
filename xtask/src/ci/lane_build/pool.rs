//! The slots one lane builds in, and the lock beside each.

use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use fs4::TryLockError;
use kithara_devtools::lock::FileLock;
use tracing::info;

use crate::ci::environment::CacheTrust;

/// The slots one lane builds in, side by side under one parent. Slot `n` is
/// `<name>-<n>`, and its lock is the file `<name>-<n>.lock` beside it: never
/// removed, so no job locks a file the budget already deleted.
#[derive(Debug)]
pub(crate) struct SlotPool {
    parent: PathBuf,
    name: String,
    /// The directory inside a slot Cargo builds in, when not the slot itself.
    build: Option<&'static str>,
}

impl SlotPool {
    /// A lane's pool on a fleet runner: `<root>/<trust>-lane-<lane>-<n>`. Trust
    /// is in the name, so a trusted build never shares a slot with a branch.
    pub(crate) fn fleet(root: &Path, trust: CacheTrust, lane: &str) -> Self {
        Self {
            parent: root.to_path_buf(),
            name: format!("{}-lane-{lane}", trust.as_str()),
            build: None,
        }
    }

    /// A lane's pool in the executor cache:
    /// `<slots>/<scope>-lane-<lane>-<n>/cargo`, where the scope already names
    /// trust and platform, and `cargo` is where every build directory of that
    /// cache keeps Cargo's output.
    pub(crate) fn executor(slots: &Path, scope: &str, lane: &str) -> Self {
        Self {
            parent: slots.to_path_buf(),
            name: format!("{scope}-lane-{lane}"),
            build: Some("cargo"),
        }
    }

    /// The first slot no job holds, with its lock held. When every slot is
    /// held, the next number is a new slot, so a claim never waits.
    pub(super) fn take(&self) -> Result<(PathBuf, FileLock)> {
        fs::create_dir_all(&self.parent)
            .with_context(|| format!("creating lane slots in {}", self.parent.display()))?;
        for index in 0..usize::MAX {
            let slot = self.parent.join(format!("{}-{index}", self.name));
            let lock = lock_of(&slot);
            let file = File::options()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&lock)
                .with_context(|| format!("opening lane slot lock {}", lock.display()))?;
            match FileLock::try_exclusive(file) {
                Ok(held) => {
                    info!("building in lane slot {}", slot.display());
                    let dir = self
                        .build
                        .map_or_else(|| slot.clone(), |build| slot.join(build));
                    return Ok((dir, held));
                }
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Error(error)) => {
                    return Err(error)
                        .with_context(|| format!("locking lane slot {}", slot.display()));
                }
            }
        }
        bail!("every lane slot in {} is held", self.parent.display())
    }
}

/// The lock file beside a lane slot.
pub(crate) fn lock_of(slot: &Path) -> PathBuf {
    let mut lock = slot.as_os_str().to_owned();
    lock.push(".lock");
    PathBuf::from(lock)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, sync::Barrier, thread};

    use super::*;

    #[test]
    fn a_held_slot_sends_the_next_job_to_the_next_slot() {
        let lanes = tempfile::tempdir().unwrap();
        let pool = SlotPool::fleet(lanes.path(), CacheTrust::Review, "test");
        let slot = |index: usize| lanes.path().join(format!("review-lane-test-{index}"));

        let (first, held) = pool.take().unwrap();
        let (second, _second) = pool.take().unwrap();
        assert_eq!(first, slot(0));
        assert_eq!(
            second,
            slot(1),
            "a job must not wait for a slot another job holds"
        );

        drop(held);
        let (again, _again) = pool.take().unwrap();
        assert_eq!(
            again,
            slot(0),
            "a released slot is the first one the next job takes"
        );
    }

    /// A push fans a lane out to several jobs that start together.
    #[test]
    fn jobs_claiming_at_once_each_get_a_slot_of_their_own() {
        const JOBS: usize = 4;
        let lanes = tempfile::tempdir().unwrap();
        let pool = SlotPool::fleet(lanes.path(), CacheTrust::Review, "test");
        let start = Barrier::new(JOBS);
        let hold = Barrier::new(JOBS);

        let taken: BTreeSet<PathBuf> = thread::scope(|scope| {
            let jobs: Vec<_> = (0..JOBS)
                .map(|_| {
                    scope.spawn(|| {
                        start.wait();
                        let (dir, lock) = pool.take().unwrap();
                        hold.wait();
                        drop(lock);
                        dir
                    })
                })
                .collect();
            jobs.into_iter().map(|job| job.join().unwrap()).collect()
        });

        let expected: BTreeSet<PathBuf> = (0..JOBS)
            .map(|index| lanes.path().join(format!("review-lane-test-{index}")))
            .collect();
        assert_eq!(
            taken, expected,
            "two jobs shared a slot, or one skipped past a free slot"
        );
    }

    /// A trusted build never reuses what a branch built, and a branch never
    /// writes into what a trusted build reuses.
    #[test]
    fn pools_of_different_trust_never_share_a_slot() {
        let lanes = tempfile::tempdir().unwrap();

        let (review, _review) = SlotPool::fleet(lanes.path(), CacheTrust::Review, "test")
            .take()
            .unwrap();
        let (trusted, _trusted) = SlotPool::fleet(lanes.path(), CacheTrust::Trusted, "test")
            .take()
            .unwrap();

        assert_eq!(review, lanes.path().join("review-lane-test-0"));
        assert_eq!(trusted, lanes.path().join("trusted-lane-test-0"));
    }

    /// The executor cache keeps Cargo's directory at `cargo` inside the slot,
    /// and the budget finds the slot's lock beside the slot.
    #[test]
    fn an_executor_slot_builds_in_cargo_and_holds_its_lock_beside_the_slot() {
        let slots = tempfile::tempdir().unwrap();
        let pool = SlotPool::executor(slots.path(), "review-macos-aarch64", "apple-lint");

        let (dir, _held) = pool.take().unwrap();

        let slot = slots.path().join("review-macos-aarch64-lane-apple-lint-0");
        assert_eq!(dir, slot.join("cargo"));
        let lock = File::options()
            .read(true)
            .write(true)
            .open(lock_of(&slot))
            .unwrap();
        assert!(matches!(
            FileLock::try_exclusive(lock),
            Err(TryLockError::WouldBlock)
        ));
    }
}
