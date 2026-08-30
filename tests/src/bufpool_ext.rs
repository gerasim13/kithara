use kithara::bufpool::{OverallBudget, PoolConfig, PoolRegion, pool_schema};

pool_schema! {
    pub TestPools {
        bytes: u8,
        samples: f32,
    }
}

pub type Pools = PoolRegion<TestPools>;

#[must_use]
pub fn pools() -> Pools {
    pools_with(
        256 * 1024 * 1024,
        PoolConfig::builder().max_buffers(usize::MAX).build(),
        PoolConfig::builder()
            .max_buffers(128)
            .trim_capacity(200_000)
            .build(),
    )
}

#[must_use]
pub fn pools_with(overall_bytes: usize, bytes: PoolConfig, samples: PoolConfig) -> Pools {
    TestPools::builder(OverallBudget(overall_bytes))
        .bytes(bytes)
        .samples(samples)
        .build()
        .unwrap_or_else(|error| panic!("test pool region: {error}"))
}
