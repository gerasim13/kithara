use bon::Builder;
use kithara_platform::sync::Arc;

use crate::{
    BytePool, SamplePool,
    budget::RegionBudget,
    global::{
        BYTE_MAX_BUFFERS, BYTE_TRIM_CAPACITY, DEFAULT_MAX_BYTES, SAMPLE_MAX_BUFFERS,
        SAMPLE_TRIM_CAPACITY,
    },
};

/// Configuration for a shared buffer-pool region.
///
/// Pool sizing policies follow the workspace defaults in `global`; the one
/// product knob is the total byte budget shared by both pools.
#[derive(Builder, Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionConfig {
    #[builder(default = DEFAULT_MAX_BYTES)]
    max_bytes: usize,
}

impl Default for RegionConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}

/// Statistics for both pools sharing a region budget.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RegionStats {
    /// Post-initialization growth events that exceeded the shared budget.
    pub budget_overshoots: u64,
    /// Total home and steal hits for the byte pool.
    pub byte_pool_hits: u64,
    /// Fresh allocations by the byte pool.
    pub byte_pool_misses: u64,
    /// Total home and steal hits for the sample pool.
    pub sample_pool_hits: u64,
    /// Fresh allocations by the sample pool.
    pub sample_pool_misses: u64,
    /// Current bytes tracked across both pools.
    pub allocated_bytes: usize,
    /// Maximum bytes available to both pools.
    pub max_bytes: usize,
}

/// Canonical owner of byte and sample pools sharing one byte budget.
#[derive(Clone)]
pub struct Region {
    inner: Arc<RegionInner>,
}

struct RegionInner {
    byte_pool: BytePool,
    sample_pool: SamplePool,
    budget: RegionBudget,
}

impl Region {
    /// Create a region with the supplied shared-budget configuration.
    #[must_use]
    pub fn new(config: RegionConfig) -> Self {
        let budget = RegionBudget::new(config.max_bytes);
        let byte_pool =
            BytePool::with_region_budget(BYTE_MAX_BUFFERS, BYTE_TRIM_CAPACITY, budget.clone());
        let sample_pool = SamplePool::with_region_budget(
            SAMPLE_MAX_BUFFERS,
            SAMPLE_TRIM_CAPACITY,
            budget.clone(),
        );
        Self {
            inner: Arc::new(RegionInner {
                byte_pool,
                sample_pool,
                budget,
            }),
        }
    }

    /// Get the region's byte-buffer pool.
    #[must_use]
    pub fn byte_pool(&self) -> BytePool {
        self.inner.byte_pool.clone()
    }

    /// Get the region's sample-buffer pool.
    #[must_use]
    pub fn sample_pool(&self) -> SamplePool {
        self.inner.sample_pool.clone()
    }

    /// Get combined budget and per-pool hit/miss statistics.
    #[must_use]
    pub fn stats(&self) -> RegionStats {
        let byte = self.inner.byte_pool.stats();
        let samples = self.inner.sample_pool.stats();
        RegionStats {
            allocated_bytes: self.inner.budget.allocated_bytes(),
            budget_overshoots: byte.budget_overshoots + samples.budget_overshoots,
            byte_pool_hits: byte.home_hits + byte.steal_hits,
            byte_pool_misses: byte.alloc_misses,
            max_bytes: self.inner.budget.max_bytes(),
            sample_pool_hits: samples.home_hits + samples.steal_hits,
            sample_pool_misses: samples.alloc_misses,
        }
    }
}

impl Default for Region {
    /// Create a top-level convenience region.
    ///
    /// Library code should receive an injected region-derived pool instead.
    fn default() -> Self {
        Self::new(RegionConfig::default())
    }
}
