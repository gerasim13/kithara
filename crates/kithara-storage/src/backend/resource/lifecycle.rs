#![forbid(unsafe_code)]

use std::{ops::Range, path::Path, sync::atomic::Ordering};

use arc_swap::Guard;
use kithara_platform::sync::Arc;

use crate::{
    StorageResult,
    backend::{resource::state::ResourceCore, traits::DriverIo},
    resource::{ResourceStatus, range_covered_by},
};

/// Whether finalizing also publishes the driver's committed snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Publish {
    Snapshot,
    Skip,
}

impl<D: DriverIo> ResourceCore<D> {
    pub(super) fn commit_inner(&self, final_len: Option<u64>) -> StorageResult<()> {
        self.finish_inner(final_len, Publish::Snapshot)
    }

    /// Commit without publishing a driver snapshot — see [`DriverIo::seal`].
    /// Readiness is announced exactly as in [`Self::commit_inner`]: waiters
    /// must wake whether or not a snapshot was published.
    pub(super) fn seal_inner(&self, final_len: Option<u64>) -> StorageResult<()> {
        self.finish_inner(final_len, Publish::Skip)
    }

    fn finish_inner(&self, final_len: Option<u64>, publish: Publish) -> StorageResult<()> {
        self.check_health()?;

        match publish {
            Publish::Snapshot => self.inner.driver.commit(final_len)?,
            Publish::Skip => self.inner.driver.seal(final_len)?,
        }
        self.inner.committed.store(true, Ordering::Release);

        {
            let mut state = self.inner.gate.lock();
            state.committed = true;
            state.final_len = final_len;
            if let Some(len) = final_len
                && len > 0
            {
                state.available.insert(0..len);
                if let Some(window) = self.inner.driver.valid_window() {
                    if window.start > 0 {
                        state.available.remove(0..window.start);
                    }
                    if window.end < len {
                        state.available.remove(window.end..len);
                    }
                }
                self.inner
                    .available_snapshot
                    .store(Arc::new(state.available.clone()));
            }
        }
        self.inner.gate.notify_all();
        // The write side pays the frees the produce-core reads parked.
        self.inner.retired.drain();

        if let Some(len) = final_len
            && let Some(observer) = self.inner.observer.as_ref()
        {
            observer.on_commit(len);
        }

        Ok(())
    }

    /// Called from the decode produce path (`phase_at` cascade), so the loaded
    /// snapshot is parked rather than dropped here: a write publishes a new
    /// generation on every chunk, and a read that races one would otherwise be
    /// its last owner and free the range tree on the audio thread.
    pub(super) fn contains_range_inner(&self, range: Range<u64>) -> bool {
        if range.is_empty() {
            return true;
        }
        // Lock-free committed fast path. A published committed snapshot covers the
        // whole `[0, committed_len)` (both drivers are linear — `valid_window()` is
        // `None`, no eviction — so a snapshot implies no gaps), so coverage reduces
        if let Some(committed_len) = self.inner.driver.committed_len() {
            return range.end <= committed_len;
        }
        let snap = self.inner.available_snapshot.load();
        let covered = range_covered_by(&snap, &range);
        self.inner.retired.retire(Guard::into_inner(snap));
        covered
    }

    pub(super) fn fail_inner(&self, reason: String) {
        {
            let mut state = self.inner.gate.lock();
            state.failed = Some(reason);
        }
        self.inner.gate.notify_all();
    }

    pub(super) fn len_inner(&self) -> Option<u64> {
        // The committed snapshot stays published across a `reactivate` so reads
        // remain consistent, so confirm the *lifecycle* is still committed (the
        // lock-free flag) before reporting its length as the resource's final
        // length. A reactivated (being-rewritten) resource has no known total.
        if self.inner.committed.load(Ordering::Acquire)
            && let Some(committed_len) = self.inner.driver.committed_len()
        {
            return Some(committed_len);
        }
        let state = self.inner.gate.lock();
        state.final_len
    }

    pub(super) fn next_gap_inner(&self, from: u64, limit: u64) -> Option<Range<u64>> {
        let state = self.inner.gate.lock();
        let total = state.final_len.unwrap_or(limit);
        let upper = limit.min(total);
        if from >= upper {
            return None;
        }
        state
            .available
            .gaps(&(from..upper))
            .next()
            .map(|gap| gap.start..gap.end.min(upper))
    }

    pub(super) fn path_inner(&self) -> Option<&Path> {
        self.inner.driver.path()
    }

    pub(super) fn reactivate_inner(&self) -> StorageResult<()> {
        if self.inner.cancel.is_cancelled() {
            return Err(crate::StorageError::Cancelled);
        }

        self.inner.driver.reactivate()?;
        self.inner.committed.store(false, Ordering::Release);

        {
            let mut state = self.inner.gate.lock();
            state.committed = false;
            state.final_len = None;
            state.failed = None;
        }
        self.inner.gate.notify_all();
        Ok(())
    }

    /// Whether dropping an uncommitted writer should mark the core failed.
    /// `false` once the resource is committed, already failed, or cancelled
    /// (cancellation is a routine shutdown, not a writer error).
    pub(super) fn should_fail_on_drop(&self) -> bool {
        if self.inner.cancel.is_cancelled() {
            return false;
        }
        let state = self.inner.gate.lock();
        !state.committed && state.failed.is_none()
    }

    pub(super) fn status_inner(&self) -> ResourceStatus {
        let state = self.inner.gate.lock();
        if let Some(ref reason) = state.failed {
            ResourceStatus::Failed(reason.clone())
        } else if state.committed {
            ResourceStatus::Committed {
                final_len: state.final_len,
            }
        } else if self.inner.cancel.is_cancelled() {
            ResourceStatus::Cancelled
        } else {
            ResourceStatus::Active
        }
    }
}
