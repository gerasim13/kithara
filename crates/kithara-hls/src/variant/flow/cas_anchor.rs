use std::{
    hint::spin_loop,
    sync::atomic::{AtomicU32, AtomicU64, Ordering, fence},
};

use super::seqlock::AnchorEntry;

/// Generation-tagged multi-writer seqlock for an optional `{segment, anchor}` demand.
/// Core seeks and off-core preparation serialize writes by CAS-acquiring an even version.
/// Short writes include the active generation, preventing lost updates between writers.
/// RT reads never spin: an odd or changed version returns `None` until the existing re-poll.
pub(in crate::variant) struct CasAnchorCell {
    segment: AtomicU32,
    /// Seqlock version: even = stable, odd = a writer owns the body. Acquired
    /// with a CAS so two concurrent writers cannot both hold it.
    version: AtomicU32,
    /// Present generation: 0 = absent, otherwise the current monotonic
    /// generation. Written only under the version lock.
    active: AtomicU64,
    anchor: AtomicU64,
    /// Monotonic generation source.
    next_gen: AtomicU64,
}

impl CasAnchorCell {
    pub(in crate::variant) const fn new() -> Self {
        Self {
            version: AtomicU32::new(0),
            active: AtomicU64::new(0),
            next_gen: AtomicU64::new(0),
            segment: AtomicU32::new(0),
            anchor: AtomicU64::new(0),
        }
    }

    pub(in crate::variant) fn clear(&self) {
        self.active.store(0, Ordering::Release);
    }

    pub(in crate::variant) fn clear_if_generation(&self, generation: u64) -> bool {
        self.active
            .compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Bails early under a writer's demand gate instead of spinning, then validates a fenced,
    /// version-sandwiched snapshot: an unchanged version and matching generation reject torn reads
    /// or entries a concurrent `clear` retired.
    pub(in crate::variant) fn load(&self) -> Option<AnchorEntry> {
        let start = self.version.load(Ordering::Acquire);
        if start & 1 != 0 {
            return None;
        }
        let generation = self.active.load(Ordering::Acquire);
        if generation == 0 {
            return None;
        }
        let segment = self.segment.load(Ordering::Relaxed);
        let anchor = self.anchor.load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        if self.version.load(Ordering::Acquire) != start {
            return None;
        }
        if self.active.load(Ordering::Acquire) != generation {
            return None;
        }
        Some(AnchorEntry {
            segment,
            anchor,
            generation,
        })
    }

    /// Wraps around 2^64 generations, treated as practically unreachable; 0 stays reserved to mean
    /// absent.
    fn next_gen(&self) -> u64 {
        let generation = self
            .next_gen
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        if generation == 0 {
            self.next_gen
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_add(1)
        } else {
            generation
        }
    }

    /// Multi-writer publish. Acquires the version with a CAS (even -> odd); a
    /// racing writer retries. `active` and the body are written under the lock,
    /// so two writers' publishes serialize and never lost-update each other.
    ///
    /// Stores `active` as 0 before writing `segment`/`anchor`, hiding the entry from `load` until
    /// the write completes, so a stale `take_if(old)` cannot observe a torn update.
    pub(in crate::variant) fn set(&self, segment: u32, anchor: u64) {
        let held = loop {
            let cur = self.version.load(Ordering::Acquire);
            if cur & 1 == 0
                && self
                    .version
                    .compare_exchange_weak(
                        cur,
                        cur.wrapping_add(1),
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
            {
                break cur.wrapping_add(1);
            }
            spin_loop();
        };
        let generation = self.next_gen();
        self.active.store(0, Ordering::Release);
        self.segment.store(segment, Ordering::Relaxed);
        self.anchor.store(anchor, Ordering::Relaxed);
        self.active.store(generation, Ordering::Release);
        self.version.store(held.wrapping_add(1), Ordering::Release);
    }
}
