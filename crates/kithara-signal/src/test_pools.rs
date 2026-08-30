use kithara_bufpool::{OverallBudget, PoolConfig, PoolRegion, SampleBuffer, pool_schema};

pool_schema! {
    pub(crate) TestPools {
        samples: f32,
    }
}

pub(crate) fn pools(max_bytes: usize) -> PoolRegion<TestPools> {
    TestPools::builder(OverallBudget(max_bytes))
        .samples(PoolConfig::builder().max_buffers(8).build())
        .build()
        .unwrap_or_else(|error| panic!("test pool region: {error}"))
}

pub(crate) fn sample_buffer(values: &[f32]) -> SampleBuffer {
    let mut buffer = pools(1024 * 1024)
        .get_with_len::<f32>(values.len())
        .unwrap_or_else(|error| panic!("test sample buffer: {error}"));
    buffer.copy_from_slice(values);
    buffer
}
