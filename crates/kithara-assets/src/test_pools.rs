pub(crate) use kithara_bufpool::testing::TestPools;
use kithara_bufpool::{ByteBuffer, OverallBudget, PoolConfig, PoolRegion};

pub(crate) fn pools() -> PoolRegion<TestPools> {
    TestPools::region(
        OverallBudget(64 * 1024 * 1024),
        PoolConfig::builder().max_buffers(128).build(),
        PoolConfig::builder().max_buffers(8).build(),
    )
    .unwrap_or_else(|error| panic!("test pool region: {error}"))
}

pub(crate) fn buffer() -> ByteBuffer {
    pools().get::<u8>()
}
