use kithara_bufpool::{OverallBudget, PoolConfig, PoolRegion, pool_schema};

pool_schema! {
    pub(crate) TestPools {
        bytes: u8,
    }
}

pub(crate) fn pools(max_bytes: usize) -> PoolRegion<TestPools> {
    TestPools::builder(OverallBudget(max_bytes))
        .bytes(PoolConfig::builder().max_buffers(32).build())
        .build()
        .unwrap_or_else(|error| panic!("test pool region: {error}"))
}

pub(crate) fn default_pools() -> PoolRegion<TestPools> {
    pools(64 * 1024 * 1024)
}
