//! Shared application-shaped schema for test composition roots.

crate::pool_schema! {
    /// Byte and sample pools available to one isolated test harness.
    pub TestPools {
        bytes: u8,
        samples: f32,
    }
}

impl TestPools {
    /// Build one byte-and-sample pool facade for an isolated test harness.
    ///
    /// # Errors
    /// Returns an error when either pool config or eager allocation is invalid.
    pub fn region(
        overall_budget: crate::OverallBudget,
        bytes: crate::PoolConfig,
        samples: crate::PoolConfig,
    ) -> Result<crate::PoolRegion<Self>, crate::PoolError> {
        Self::builder(overall_budget)
            .bytes(bytes)
            .samples(samples)
            .build()
    }
}
