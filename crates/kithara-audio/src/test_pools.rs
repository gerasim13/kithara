use kithara_bufpool::{OverallBudget, PoolConfig, PoolRegion, SampleBuffer, pool_schema};

pool_schema! {
    pub(crate) TestPools {
        bytes: u8,
        samples: f32,
    }
}

pub(crate) fn pools() -> PoolRegion<TestPools> {
    let config = || PoolConfig::builder().max_buffers(256).build();
    TestPools::builder(OverallBudget(256 * 1024 * 1024))
        .bytes(config())
        .samples(config())
        .build()
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
