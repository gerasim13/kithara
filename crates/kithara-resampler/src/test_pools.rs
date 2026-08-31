pub(crate) use kithara_bufpool::testing::TestPools;
use kithara_bufpool::{OverallBudget, PoolConfig, PoolRegion};

pub(crate) fn pools() -> PoolRegion<TestPools> {
    TestPools::region(
        OverallBudget(64 * 1024 * 1024),
        PoolConfig::builder().max_buffers(32).build(),
        PoolConfig::builder().max_buffers(64).build(),
    )
    .unwrap_or_else(|error| panic!("test pool region: {error}"))
}
