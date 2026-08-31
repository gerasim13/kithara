pub(crate) use kithara_bufpool::testing::TestPools;
use kithara_bufpool::{OverallBudget, PoolConfig, PoolRegion, SampleBuffer};

pub(crate) fn pools(max_bytes: usize) -> PoolRegion<TestPools> {
    TestPools::region(
        OverallBudget(max_bytes),
        PoolConfig::builder().max_buffers(32).build(),
        PoolConfig::builder().max_buffers(64).build(),
    )
    .unwrap_or_else(|error| panic!("test pool region: {error}"))
}

pub(crate) fn default_pools() -> PoolRegion<TestPools> {
    pools(256 * 1024 * 1024)
}

pub(crate) fn sample_buffer(values: &[f32]) -> SampleBuffer {
    let mut buffer = default_pools()
        .get_with_len::<f32>(values.len())
        .unwrap_or_else(|error| panic!("test sample buffer: {error}"));
    buffer.copy_from_slice(values);
    buffer
}
