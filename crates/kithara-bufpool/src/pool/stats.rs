/// Reuse counters for one physical pool slot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PoolStats {
    /// Acquisitions that created a fresh empty buffer.
    pub alloc_misses: u64,
    /// Acquisitions served by the caller thread's shard.
    pub home_hits: u64,
    /// Returned buffers rejected by their shard.
    pub put_drops: u64,
    /// Acquisitions served by a different thread shard.
    pub steal_hits: u64,
}
