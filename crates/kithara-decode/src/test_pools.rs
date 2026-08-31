pub(crate) use kithara_bufpool::testing::TestPools;
use kithara_bufpool::{OverallBudget, PoolConfig, PoolRegion};

pub(crate) fn pools(max_bytes: usize) -> PoolRegion<TestPools> {
    let config = || PoolConfig::builder().max_buffers(256).build();
    TestPools::region(OverallBudget(max_bytes), config(), config())
        .unwrap_or_else(|error| panic!("test pool region: {error}"))
}

pub(crate) fn default_pools() -> PoolRegion<TestPools> {
    pools(256 * 1024 * 1024)
}
