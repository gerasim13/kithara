pub(crate) use kithara_bufpool::testing::TestPools;
use kithara_bufpool::{OverallBudget, PoolConfig, PoolRegion};

pub(crate) fn pools() -> PoolRegion<TestPools> {
    TestPools::region(
        OverallBudget(64 * 1024 * 1024),
        PoolConfig::builder().max_buffers(256).build(),
        PoolConfig::builder().max_buffers(8).build(),
    )
    .unwrap_or_else(|error| panic!("test pool region: {error}"))
}
