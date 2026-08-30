use std::num::NonZeroUsize;

#[cfg(not(target_arch = "wasm32"))]
use kithara_platform::sync::Arc;
use kithara_platform::{CancelToken, tokio::runtime::Handle};

/// Shared resources and cancellation parent for a [`Worker`](crate::Worker).
#[non_exhaustive]
#[derive(Clone, fieldwork::Fieldwork)]
#[fieldwork(opt_in, with)]
pub struct WorkerConfig {
    #[field(with)]
    pub(crate) max_compute_tasks: NonZeroUsize,
    #[field(with, option_set_some)]
    pub(crate) cancel: Option<CancelToken>,
    #[field(with, option_set_some)]
    pub(crate) runtime: Option<Handle>,
    pub(crate) pool: PoolConfig,
}

impl WorkerConfig {
    /// Create a standalone worker with no Tokio handle or Rayon pool.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            cancel: None,
            max_compute_tasks: NonZeroUsize::MIN,
            pool: PoolConfig::Disabled,
            runtime: None,
        }
    }

    /// Lazily create an owned Rayon pool on the first admitted compute job.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn with_owned_pool(mut self, config: RayonConfig) -> Self {
        self.pool = PoolConfig::OwnedLazy(config);
        self
    }

    /// Share an existing Rayon pool without creating another pool.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn with_pool(mut self, pool: Arc<rayon::ThreadPool>) -> Self {
        self.pool = PoolConfig::Shared(pool);
        self
    }
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub(crate) enum PoolConfig {
    Disabled,
    #[cfg(not(target_arch = "wasm32"))]
    OwnedLazy(RayonConfig),
    #[cfg(not(target_arch = "wasm32"))]
    Shared(Arc<rayon::ThreadPool>),
}

/// Configuration for a Rayon pool built on first admitted compute work.
#[cfg(not(target_arch = "wasm32"))]
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RayonConfig {
    pub(crate) threads: NonZeroUsize,
    pub(crate) name: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl RayonConfig {
    /// Configure the thread count and thread-name prefix.
    #[must_use]
    pub fn new<N: Into<String>>(threads: NonZeroUsize, name: N) -> Self {
        Self {
            threads,
            name: name.into(),
        }
    }
}
