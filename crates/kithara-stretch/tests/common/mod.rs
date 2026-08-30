use kithara_bufpool::{OverallBudget, PoolConfig, PoolRegion, pool_schema};

pool_schema! {
    pub(crate) TestPools {
        samples: f32,
    }
}

pub(crate) fn pools(max_bytes: usize) -> PoolRegion<TestPools> {
    TestPools::builder(OverallBudget(max_bytes))
        .samples(PoolConfig::builder().max_buffers(64).build())
        .build()
        .unwrap_or_else(|error| panic!("test pool region: {error}"))
}

pub(crate) fn default_pools() -> PoolRegion<TestPools> {
    pools(256 * 1024 * 1024)
}
