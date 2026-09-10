use kithara_platform::sync::{Arc, OnceLock};
use rayon::ThreadPoolBuilder;

use super::ComputeSubmitError;
use crate::{OwnedPoolConfig, config::PoolConfig};

pub(crate) enum ComputePool {
    Disabled,
    OwnedLazy {
        config: OwnedPoolConfig,
        pool: OnceLock<Result<Arc<rayon::ThreadPool>, String>>,
    },
    Shared(Arc<rayon::ThreadPool>),
}

impl ComputePool {
    pub(super) fn new(config: PoolConfig) -> Self {
        match config {
            PoolConfig::Disabled => Self::Disabled,
            PoolConfig::OwnedLazy(config) => Self::OwnedLazy {
                config,
                pool: OnceLock::new(),
            },
            PoolConfig::Shared(pool) => Self::Shared(pool),
        }
    }

    pub(super) const fn is_disabled(&self) -> bool {
        matches!(self, Self::Disabled)
    }

    #[cfg(test)]
    pub(crate) fn owned_is_initialized(&self) -> bool {
        matches!(self, Self::OwnedLazy { pool, .. } if pool.get().is_some())
    }

    #[cfg(test)]
    pub(crate) fn shared(&self) -> Option<&Arc<rayon::ThreadPool>> {
        let Self::Shared(pool) = self else {
            return None;
        };
        Some(pool)
    }

    pub(super) fn spawner(&self) -> Result<Spawner, ComputeSubmitError> {
        match self {
            Self::Disabled => Err(ComputeSubmitError::Unavailable),
            Self::Shared(pool) => Ok(Spawner(Arc::clone(pool))),
            Self::OwnedLazy { config, pool } => pool
                .get_or_init(|| build_pool(config))
                .as_ref()
                .map(|pool| Spawner(Arc::clone(pool)))
                .map_err(|_| ComputeSubmitError::Unavailable),
        }
    }
}

pub(super) struct Spawner(Arc<rayon::ThreadPool>);

impl Spawner {
    pub(super) fn spawn<F: FnOnce() + Send + 'static>(self, job: F) {
        self.0.spawn(job);
    }
}

fn build_pool(config: &OwnedPoolConfig) -> Result<Arc<rayon::ThreadPool>, String> {
    let prefix = config.name.clone();
    ThreadPoolBuilder::new()
        .num_threads(config.threads.get())
        .thread_name(move |index| format!("{prefix}-{index}"))
        .build()
        .map(Arc::new)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use kithara_platform::{
        CancelScope,
        sync::{Arc, OnceLock},
    };
    use kithara_test_utils::kithara;

    use super::ComputePool;
    use crate::{
        OwnedPoolConfig, Wake,
        compute::{Budget, ComputeRuntime, ComputeSubmitError},
    };

    #[kithara::test(native, flash(false))]
    fn owned_pool_failure_returns_payload_and_releases_both_permits() {
        let failed = OnceLock::new();
        assert!(failed.set(Err(String::from("pool build failed"))).is_ok());
        let runtime = ComputeRuntime {
            budget: Arc::new(Budget::new(std::num::NonZeroUsize::MIN)),
            pool: ComputePool::OwnedLazy {
                config: OwnedPoolConfig::new(std::num::NonZeroUsize::MIN, "failed-pool-test"),
                pool: failed,
            },
        };
        let task_budget = Arc::new(Budget::new(std::num::NonZeroUsize::MIN));
        let scope = CancelScope::new(None);
        let token = scope.token().child();
        let rejected = runtime
            .submit(
                &task_budget,
                &token,
                Wake::default(),
                String::from("detector"),
                |_, _| {},
            )
            .expect_err("cached pool build failure must reject compute");

        assert_eq!(rejected.reason(), ComputeSubmitError::Unavailable);
        assert_eq!(rejected.recover_payload(), "detector");
        assert_eq!(task_budget.active(), 0);
        assert_eq!(runtime.budget.active(), 0);
    }
}
