/// ```compile_fail
/// use kithara_bufpool::{OverallBudget, PoolConfig, pool_schema};
/// pool_schema! { pub MissingPools { bytes: u8, samples: f32 } }
/// let config = PoolConfig::builder().max_buffers(8).build();
/// let _ = MissingPools::builder(OverallBudget(64)).bytes(config).build();
/// ```
mod missing_registration {}

/// ```compile_fail
/// use kithara_bufpool::{OverallBudget, PoolConfig, pool_schema};
/// pool_schema! { pub DuplicatePools { bytes: u8 } }
/// let config = || PoolConfig::builder().max_buffers(8).build();
/// let _ = DuplicatePools::builder(OverallBudget(64))
///     .bytes(config())
///     .bytes(config());
/// ```
mod duplicate_registration {}

/// ```compile_fail
/// use kithara_bufpool::{OverallBudget, PoolConfig, pool_schema};
/// pool_schema! { pub KnownPools { bytes: u8 } }
/// let config = PoolConfig::builder().max_buffers(8).build();
/// let _ = KnownPools::builder(OverallBudget(64)).samples(config);
/// ```
mod unknown_registration {}

/// ```compile_fail
/// use kithara_bufpool::{OverallBudget, PoolConfig, pool_schema};
/// pool_schema! { pub BytePools { bytes: u8 } }
/// let pools = BytePools::builder(OverallBudget(64))
///     .bytes(PoolConfig::builder().max_buffers(8).build())
///     .build()
///     .unwrap();
/// let _ = pools.get::<f32>();
/// ```
mod unregistered_key {}
