use std::{
    hint::spin_loop,
    sync::atomic::{AtomicU32, AtomicU64, Ordering, fence},
};

use super::seqlock::AnchorEntry;

/// A generation-tagged MULTI-writer seqlock cell holding `{segment, anchor}` plus a
/// present/absent generation. Unlike [`SeqAnchorCell`](super::seqlock::SeqAnchorCell)
/// (a single on-core body writer), the exact-seek demand is body-written from BOTH the
/// produce-core seek path (`seek_time_anchor`) and the off-RT profiled reader
/// preparation, with no lock shared between them. The version is therefore acquired
/// with a CAS (even -> odd): the two writers serialize, the loser retries — writes are
/// short (five atomic stores, no alloc/lock/log) and rare. The RT read path never spins
/// on a write-in-flight: an odd or changed version returns `None` (not-ready) for this
/// poll and relies on the existing level-triggered re-poll (`SchedulerWake` /
/// `WAITING_TIMEOUT` / next `read_at`) to observe the demand a tick later — the
/// non-blocking analog of the original `Mutex`'s blocking wait. `active` is the present
/// generation (0 = `None`), published *inside* the version critical section so two
/// writers' publishes can never lost-update each other.
pub(super) struct CasAnchorCell {
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
    pub(super) const fn new() -> Self {
        Self {
            version: AtomicU32::new(0),
            active: AtomicU64::new(0),
            next_gen: AtomicU64::new(0),
            segment: AtomicU32::new(0),
            anchor: AtomicU64::new(0),
        }
    }

    pub(super) fn clear(&self) {
        self.active.store(0, Ordering::Release);
    }

    pub(super) fn clear_if_generation(&self, generation: u64) -> bool {
        self.active
            .compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Bails early under a writer's demand gate instead of spinning, then validates a fenced,
    /// version-sandwiched snapshot: an unchanged version and matching generation reject torn reads
    /// or entries a concurrent `clear` retired.
    pub(super) fn load(&self) -> Option<AnchorEntry> {
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
    pub(super) fn set(&self, segment: u32, anchor: u64) {
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
