pub(crate) use kithara_bufpool::testing::TestPools;
use kithara_bufpool::{OverallBudget, PoolConfig, PoolRegion};

pub(crate) fn pools(max_bytes: usize) -> PoolRegion<TestPools> {
    TestPools::region(
        OverallBudget(max_bytes),
        PoolConfig::builder().max_buffers(32).build(),
        PoolConfig::builder().max_buffers(8).build(),
    )
    .unwrap_or_else(|error| panic!("test pool region: {error}"))
}

pub(crate) fn default_pools() -> PoolRegion<TestPools> {
    pools(64 * 1024 * 1024)
}
