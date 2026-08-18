#![forbid(unsafe_code)]

use std::{
    mem,
    sync::atomic::{AtomicBool, Ordering},
};

use crossbeam_queue::ArrayQueue;
use kithara_platform::sync::Arc;
use tracing::warn;

use super::core::{AssetTree, Availability};

/// Snapshot references parked by produce-core reads for off-RT reclaim.
///
/// A snapshot guard dropped on the audio thread can be the last owner of a
/// generation a writer has already replaced, and freeing the tree there is a
/// real-time violation (`RTSan`: unsafe-library-call in `AvailabilityIndex`
/// reads). Readers therefore never drop a snapshot they own: they park it
/// here, and the write side — the index's download and deletion paths, which
/// already publish the generations — drains the bin and pays the frees.
/// Overflow leaks the reference instead of freeing on the reader, mirroring
/// the decode retire queue; it only happens when writers are idle, which is
/// exactly when generations are not being replaced and the leak pins memory
/// that is live anyway.
pub(super) struct Retired {
    trees: ArrayQueue<Arc<AssetTree>>,
    availabilities: ArrayQueue<Arc<Availability>>,
    overflowed: AtomicBool,
}

/// Capacity of each retire queue. Reads park at produce-tick cadence and
/// write-side drains run at download cadence; 256 spans that gap with room,
/// and overflow degrades to a leak, never to a free on the reader.
pub(super) const RETIRE_CAPACITY: usize = 256;

impl Retired {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            trees: ArrayQueue::new(capacity),
            availabilities: ArrayQueue::new(capacity),
            overflowed: AtomicBool::new(false),
        }
    }

    pub(super) fn drain(&self) {
        while self.trees.pop().is_some() {}
        while self.availabilities.pop().is_some() {}
        if self.overflowed.swap(false, Ordering::AcqRel) {
            warn!("availability retire bin overflowed; leaked snapshots to keep the reader free");
        }
    }

    pub(super) fn retire_tree(&self, tree: Arc<AssetTree>) {
        if let Err(tree) = self.trees.push(tree) {
            self.overflowed.store(true, Ordering::Release);
            mem::forget(tree);
        }
    }

    pub(super) fn retire_availability(&self, availability: Arc<Availability>) {
        if let Err(availability) = self.availabilities.push(availability) {
            self.overflowed.store(true, Ordering::Release);
            mem::forget(availability);
        }
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.trees.is_empty() && self.availabilities.is_empty()
    }
}
