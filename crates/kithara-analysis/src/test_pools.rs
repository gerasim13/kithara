pub(crate) use kithara_bufpool::testing::TestPools;
use kithara_bufpool::{OverallBudget, PoolConfig, PoolRegion, SampleBuffer};

pub(crate) fn pools() -> PoolRegion<TestPools> {
    let config = || PoolConfig::builder().max_buffers(2_048).build();
    TestPools::region(OverallBudget(512 * 1024 * 1024), config(), config())
        .unwrap_or_else(|error| panic!("test pool region: {error}"))
}

pub(crate) fn sample_buffer(values: &[f32]) -> SampleBuffer {
    let pools = pools();
    let mut buffer = pools
        .get_with_len::<f32>(values.len())
        .unwrap_or_else(|error| panic!("test sample buffer: {error}"));
    buffer.copy_from_slice(values);
    buffer
}
