use std::num::NonZeroUsize;

#[cfg(not(target_arch = "wasm32"))]
use kithara_platform::sync::Arc;
use kithara_platform::{CancelToken, tokio::runtime::Handle};
use serde::Deserialize;
use struct_patch::Patch;

/// Shared resources and cancellation parent for a [`Worker`](crate::Worker).
#[non_exhaustive]
#[derive(Clone, fieldwork::Fieldwork, Patch)]
#[fieldwork(opt_in, with)]
#[patch(name = "WorkerSettings")]
#[patch(attribute(derive(Clone, Debug, Default, Deserialize)))]
#[patch(attribute(serde(default, deny_unknown_fields)))]
#[patch(attribute(non_exhaustive))]
pub struct WorkerConfig {
    #[field(with)]
    pub(crate) max_compute_tasks: NonZeroUsize,
    #[field(with, option_set_some)]
    #[patch(skip)]
    pub(crate) cancel: Option<CancelToken>,
    #[field(with, option_set_some)]
    #[patch(skip)]
    pub(crate) runtime: Option<Handle>,
    #[patch(skip)]
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

/// What a document can say about the compute pool. `Shared` is absent on
/// purpose: it carries a live `rayon::ThreadPool` only code can hand over.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields, tag = "mode")]
#[non_exhaustive]
pub enum ComputePoolSettings {
    Disabled,
    #[cfg(not(target_arch = "wasm32"))]
    Owned {
        name: String,
        threads: NonZeroUsize,
    },
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::num::NonZeroUsize;

    use kithara_test_utils::kithara;
    use struct_patch::Patch as _;

    use super::{ComputePoolSettings, PoolConfig, RayonConfig, WorkerConfig, WorkerSettings};

    #[kithara::test(native, flash(false))]
    fn a_patch_writes_only_the_field_it_names() {
        let seeded_pool = RayonConfig::new(NonZeroUsize::new(3).expect("nonzero"), "seed");
        let mut config = WorkerConfig::new()
            .with_max_compute_tasks(NonZeroUsize::new(2).expect("nonzero"))
            .with_owned_pool(seeded_pool.clone());

        let patch: WorkerSettings =
            serde_yaml_ng::from_str("max_compute_tasks: 4\n").expect("valid patch document");
        config.apply(patch);

        assert_eq!(config.max_compute_tasks.get(), 4);
        match &config.pool {
            PoolConfig::OwnedLazy(pool) => assert_eq!(
                *pool, seeded_pool,
                "an unnamed field keeps its seeded value"
            ),
            PoolConfig::Disabled | PoolConfig::Shared(_) => {
                panic!("pool must keep the seeded OwnedLazy variant")
            }
        }
    }

    #[kithara::test(native, flash(false))]
    fn compute_pool_settings_owned_parses_name_and_threads() {
        let settings: ComputePoolSettings =
            serde_yaml_ng::from_str("mode: owned\nname: analysis\nthreads: 2\n")
                .expect("a valid owned-pool document parses");

        match settings {
            ComputePoolSettings::Owned { name, threads } => {
                assert_eq!(name, "analysis");
                assert_eq!(threads.get(), 2);
            }
            ComputePoolSettings::Disabled => panic!("expected the owned variant"),
        }
    }

    #[kithara::test(native, flash(false))]
    fn compute_pool_settings_rejects_a_shared_mode() {
        let error = serde_yaml_ng::from_str::<ComputePoolSettings>("mode: shared\n")
            .expect_err("a document cannot name a live pool it does not own");

        assert!(error.to_string().contains("shared"), "{error}");
    }
}
