pub(crate) use kithara_bufpool::testing::TestPools;
use kithara_bufpool::{OverallBudget, PoolConfig, PoolRegion, SampleBuffer};

pub(crate) fn pools() -> PoolRegion<TestPools> {
    TestPools::region(
        OverallBudget(64 * 1024 * 1024),
        PoolConfig::builder().max_buffers(32).build(),
        PoolConfig::builder().max_buffers(128).build(),
    )
    .unwrap_or_else(|error| panic!("test pool region: {error}"))
}

pub(crate) fn filled(len: usize, value: f32) -> SampleBuffer {
    let pools = pools();
    let mut samples = pools
        .get_with_len::<f32>(len)
        .unwrap_or_else(|error| panic!("test sample buffer: {error}"));
    samples.fill(value);
    samples
}
