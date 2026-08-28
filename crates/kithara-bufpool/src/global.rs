use std::{
    fmt,
    ops::{Deref, DerefMut, RangeBounds},
    sync::OnceLock,
};

use crate::{
    BudgetExhausted, ByteBudget,
    budget::RegionBudget,
    pool::{PoolStats, PooledOwned, SharedPool},
};

pub(crate) const BYTE_MAX_BUFFERS: usize = usize::MAX;
pub(crate) const BYTE_TRIM_CAPACITY: usize = 0;
/// Workspace default byte budget: the cap a `Region` shares between its two
/// pools, and the cap on the process-wide `BytePool` singleton. One number,
/// because the two describe the same "how much memory may pooling hold".
pub(crate) const DEFAULT_MAX_BYTES: usize = 256 * 1024 * 1024;
pub(crate) const SAMPLE_MAX_BUFFERS: usize = 128;
pub(crate) const SAMPLE_TRIM_CAPACITY: usize = 200_000;

/// Standard byte buffer pool type for the entire workspace.
///
/// `BytePool::default()` returns a clone of a process-wide `OnceLock`-backed
/// instance — cheap (one `Arc::clone`) and produces a singleton across the
/// program. Top-level entry points (main, FFI) build one pool here and pass
/// it down through their config structs; library code should never call
/// `BytePool::default()` itself — read the pool from injected config.
pub type BytePool = SharedPool<32, Vec<u8>>;

/// Standard decoded-sample (`f32`) buffer pool for the entire workspace.
///
/// Uses 8 shards (128 buffers / 8 = 16 per shard) for good single-thread
/// reuse without excessive cross-shard stealing. Same `Default` policy as
/// `BytePool`.
#[derive(Clone, Debug)]
pub struct SamplePool(SharedPool<8, Vec<f32>>);

impl SamplePool {
    /// Create a new shared sample pool.
    #[must_use]
    pub fn new(max_buffers: usize, trim_capacity: usize) -> Self {
        Self(SharedPool::new(max_buffers, trim_capacity))
    }

    delegate::delegate! {
        to self.0 {
            /// Current number of tracked bytes across all live sample buffers.
            #[must_use]
            pub fn allocated_bytes(&self) -> usize;
            /// Wrap an already charged sample buffer for automatic recycling.
            #[must_use]
            #[expr(SampleBuffer($))]
            pub fn attach(&self, value: Vec<f32>) -> SampleBuffer;
            /// Get a sample buffer from the shared pool.
            #[must_use]
            #[expr(SampleBuffer($))]
            pub fn get(&self) -> SampleBuffer;
            /// Return a sample buffer to the pool for reuse.
            pub fn recycle(&self, value: Vec<f32>);
            /// Get pool hit/miss statistics.
            #[must_use]
            pub fn stats(&self) -> PoolStats;
        }
    }

    /// Get a sample buffer with initialization.
    pub fn get_with<F>(&self, init: F) -> SampleBuffer
    where
        F: FnOnce(&mut Vec<f32>),
    {
        SampleBuffer(self.0.get_with(init))
    }

    /// Collect samples directly into a buffer owned by this pool.
    #[must_use]
    pub fn collect<I>(&self, samples: I) -> SampleBuffer
    where
        I: IntoIterator<Item = f32>,
    {
        SampleBuffer(self.0.collect(samples))
    }

    /// Pre-warm the pool by creating and recycling `count` buffers.
    pub fn pre_warm<F>(&self, count: usize, init: F)
    where
        F: Fn(&mut Vec<f32>),
    {
        self.0.pre_warm(count, init);
    }

    /// Create a shared sample pool with a byte budget limit.
    #[must_use]
    pub fn with_byte_budget(max_buffers: usize, trim_capacity: usize, budget: ByteBudget) -> Self {
        Self(SharedPool::with_byte_budget(
            max_buffers,
            trim_capacity,
            budget,
        ))
    }

    pub(crate) fn with_region_budget(
        max_buffers: usize,
        trim_capacity: usize,
        budget: RegionBudget,
    ) -> Self {
        Self(SharedPool::with_region_budget(
            max_buffers,
            trim_capacity,
            budget,
        ))
    }
}

/// Pooled sample buffer that auto-recycles to the source pool on drop.
///
/// Use this instead of `Vec<f32>` in audio pipelines to enable
/// zero-allocation buffer reuse.
pub struct SampleBuffer(PooledOwned<8, Vec<f32>>);

impl SampleBuffer {
    delegate::delegate! {
        to self.0 {
            /// Return the allocated sample capacity.
            #[must_use]
            pub fn capacity(&self) -> usize;
            /// Remove all samples while retaining the allocated capacity.
            pub fn clear(&mut self);
            /// Grow the buffer to at least `min_len` samples under its byte budget.
            ///
            /// # Errors
            ///
            /// Returns [`BudgetExhausted`] if the growth exceeds the pool's byte
            /// budget or the requested capacity cannot be reserved.
            pub fn ensure_len(&mut self, min_len: usize) -> Result<(), BudgetExhausted>;
            /// Extract the sample vector without returning it to the pool.
            #[must_use]
            pub fn into_inner(self) -> Vec<f32>;
            /// Remove consecutive duplicate samples without growing the buffer.
            pub fn dedup(&mut self);
            /// Retain only samples matching `keep` without growing the buffer.
            pub fn retain<F>(&mut self, keep: F)
            where
                F: FnMut(&f32) -> bool;
            /// Shorten the buffer to `len` samples.
            pub fn truncate(&mut self, len: usize);
        }
    }

    /// Remove and yield the specified sample range.
    pub fn drain<R>(&mut self, range: R) -> std::vec::Drain<'_, f32>
    where
        R: RangeBounds<usize>,
    {
        self.0.drain(range)
    }
}

impl Deref for SampleBuffer {
    type Target = [f32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for SampleBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl fmt::Debug for SampleBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Default-constructed `BytePool` returns a process-wide singleton with a
/// 256 MB byte budget and no buffer-count limit (the budget is the cap).
/// Trim is disabled — buffers always grow up to their high-water mark.
impl Default for BytePool {
    fn default() -> Self {
        static GLOBAL: OnceLock<BytePool> = OnceLock::new();
        const BUDGET: ByteBudget = ByteBudget(DEFAULT_MAX_BYTES);
        GLOBAL
            .get_or_init(|| Self::with_byte_budget(BYTE_MAX_BUFFERS, BYTE_TRIM_CAPACITY, BUDGET))
            .clone()
    }
}

/// Default-constructed `SamplePool` returns a process-wide singleton with at
/// most 128 buffers and a 200 000-element trim cap.
impl Default for SamplePool {
    fn default() -> Self {
        static GLOBAL: OnceLock<SamplePool> = OnceLock::new();
        GLOBAL
            .get_or_init(|| Self::new(SAMPLE_MAX_BUFFERS, SAMPLE_TRIM_CAPACITY))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    /// The reason `pre_warm` exists: the buffers a decode asks for come back
    /// from the free list instead of allocating, and allocating is what the
    /// audio thread may not do. Measured on a pool nobody else holds, because
    /// the property is the warm's, not a moment's.
    #[kithara::test(native, flash(false))]
    fn a_warmed_pool_serves_what_it_was_warmed_for_without_allocating() {
        let pool = SamplePool::new(128, 200_000);
        let samples = 4_608 * 2;
        pool.pre_warm(8, |buffer| {
            buffer.clear();
            buffer.resize(samples, 0.0);
        });

        let misses_before = pool.stats().alloc_misses;
        let buffers: Vec<_> = (0..8)
            .map(|_| pool.get_with(|buffer| buffer.resize(samples, 0.0)))
            .collect();

        assert_eq!(pool.stats().alloc_misses, misses_before);
        drop(buffers);
    }

    /// One more than it was warmed for has to come from somewhere.
    #[kithara::test(native, flash(false))]
    fn a_warmed_pool_still_allocates_past_its_warm() {
        let pool = SamplePool::new(128, 200_000);
        let samples = 4_608 * 2;
        pool.pre_warm(2, |buffer| {
            buffer.clear();
            buffer.resize(samples, 0.0);
        });

        let misses_before = pool.stats().alloc_misses;
        let buffers: Vec<_> = (0..3)
            .map(|_| pool.get_with(|buffer| buffer.resize(samples, 0.0)))
            .collect();

        assert_eq!(pool.stats().alloc_misses, misses_before.saturating_add(1));
        drop(buffers);
    }

    /// A warm that hands back buffers too small for a decode is no warm at
    /// all: the first `resize` reallocates on the very path the pool exists to
    /// keep allocation-free.
    #[kithara::test(native, flash(false))]
    fn a_warmed_buffer_is_already_the_size_it_was_warmed_to() {
        let pool = SamplePool::new(128, 200_000);
        let samples = 4_608 * 2;
        pool.pre_warm(1, |buffer| {
            buffer.clear();
            buffer.resize(samples, 0.0);
        });

        let buffer = pool.get_with(|buffer| buffer.clear());

        assert!(buffer.capacity() >= samples, "{}", buffer.capacity());
    }

    #[kithara::test(native, flash(false))]
    fn collect_fills_sample_and_byte_buffers_without_intermediate_storage() {
        let samples = SamplePool::new(8, 32).collect([0.25, -0.5, 1.0]);
        assert_eq!(&*samples, &[0.25, -0.5, 1.0]);

        let bytes = BytePool::new(8, 32).collect([1_u8, 2, 3]);
        assert_eq!(&*bytes, &[1, 2, 3]);
    }

    #[kithara::test(native, flash(false))]
    fn collect_returns_an_empty_pool_guard_for_an_empty_iterator() {
        let samples = SamplePool::new(8, 32).collect(std::iter::empty());
        assert!(samples.is_empty());
    }

    #[kithara::test(native, flash(false))]
    fn collect_reuses_a_returned_sample_buffer() {
        let pool = SamplePool::new(8, 32);
        drop(pool.collect(std::iter::repeat_n(0.25, 16)));
        let before = pool.stats();

        let samples = pool.collect([1.0, -1.0]);
        let after = pool.stats();

        assert_eq!(&*samples, &[1.0, -1.0]);
        assert_eq!(after.alloc_misses, before.alloc_misses);
        assert!(
            after.home_hits + after.steal_hits > before.home_hits + before.steal_hits,
            "collect did not reuse a returned sample buffer"
        );
    }

    #[kithara::test(native, flash(false))]
    fn collect_returns_growth_and_records_a_budget_overshoot() {
        let pool = SamplePool::with_byte_budget(8, 32, ByteBudget(0));

        let samples = pool.collect([0.5]);
        let stats = pool.stats();

        assert_eq!(&*samples, &[0.5]);
        assert_eq!(stats.budget_overshoots, 1);
        assert!(stats.allocated_bytes > 0);
    }
}
