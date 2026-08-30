use kithara_bufpool::{OverallBudget, PoolConfig, PoolRegion, SampleBuffer, pool_schema};

pool_schema! {
    pub(crate) TestPools {
        samples: f32,
    }
}

pub(crate) fn pools() -> PoolRegion<TestPools> {
    TestPools::builder(OverallBudget(64 * 1024 * 1024))
        .samples(PoolConfig::builder().max_buffers(128).build())
        .build()
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
