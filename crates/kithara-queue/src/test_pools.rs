pub(crate) use kithara_bufpool::testing::TestPools;
use kithara_bufpool::{OverallBudget, PoolConfig, PoolRegion};

pub(crate) fn pools() -> PoolRegion<TestPools> {
    let config = || PoolConfig::builder().max_buffers(256).build();
    TestPools::region(OverallBudget(256 * 1024 * 1024), config(), config())
        .unwrap_or_else(|error| panic!("test pool region: {error}"))
}
