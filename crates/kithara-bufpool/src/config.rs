use bon::Builder;

use crate::Percent;

/// Policy for one physical buffer pool in a region.
#[derive(Builder, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolConfig {
    /// Number of reusable payloads allocated during region construction.
    #[builder(default)]
    pub(crate) initial_buffers: usize,
    /// Element capacity of each initially allocated payload.
    #[builder(default)]
    pub(crate) initial_capacity: usize,
    /// Maximum number of retained buffers across all shards.
    pub(crate) max_buffers: usize,
    /// Drop returned buffers above this capacity. Zero disables the ceiling.
    #[builder(default)]
    pub(crate) max_retained_capacity: usize,
    /// Maximum share of the region budget this pool may hold.
    #[builder(default = Percent::FULL)]
    pub(crate) max_share: Percent,
    /// Capacity retained when an oversized buffer returns to the pool.
    #[builder(default)]
    pub(crate) trim_capacity: usize,
}
