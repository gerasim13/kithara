use kithara_bufpool::{ByteBuffer, OverallBudget, PoolConfig, PoolRegion, pool_schema};

pool_schema! {
    pub(crate) TestPools {
        bytes: u8,
    }
}

pub(crate) fn pools() -> PoolRegion<TestPools> {
    TestPools::builder(OverallBudget(64 * 1024 * 1024))
        .bytes(PoolConfig::builder().max_buffers(128).build())
        .build()
        .unwrap_or_else(|error| panic!("test pool region: {error}"))
}

pub(crate) fn buffer() -> ByteBuffer {
    pools().get::<u8>()
}
