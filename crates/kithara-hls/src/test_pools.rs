use kithara_bufpool::{OverallBudget, PoolConfig, PoolRegion, pool_schema};

pool_schema! {
    pub(crate) TestPools {
        bytes: u8,
    }
}

pub(crate) fn pools() -> PoolRegion<TestPools> {
    TestPools::builder(OverallBudget(64 * 1024 * 1024))
        .bytes(PoolConfig::builder().max_buffers(256).build())
        .build()
        .unwrap_or_else(|error| panic!("test pool region: {error}"))
}
