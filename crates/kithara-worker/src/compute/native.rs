use kithara_platform::sync::{Arc, OnceLock};
use rayon::ThreadPoolBuilder;

use super::ComputeSubmitError;
use crate::{OwnedPoolConfig, config::PoolConfig};

pub(crate) struct ComputePool {
    pub(crate) owned: OnceLock<Result<Arc<rayon::ThreadPool>, String>>,
}

impl ComputePool {
    pub(super) const fn new() -> Self {
        Self {
            owned: OnceLock::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn owned_is_initialized(&self) -> bool {
        self.owned.get().is_some()
    }

    pub(super) fn spawner(&self, config: &PoolConfig) -> Result<Spawner, ComputeSubmitError> {
        match config {
            PoolConfig::Disabled => Err(ComputeSubmitError::Unavailable),
            PoolConfig::Shared(pool) => Ok(Spawner(Arc::clone(pool))),
            PoolConfig::OwnedLazy(config) => self
                .owned
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
    use std::num::NonZeroUsize;

    use kithara_platform::{
        CancelScope,
        sync::{Arc, OnceLock, mpsc},
        time::{Duration, Instant},
    };
    use kithara_test_utils::kithara;

    use super::ComputePool;
    use crate::{
        OwnedPoolConfig, Wake, WorkerConfig,
        compute::{Budget, ComputeRuntime, ComputeSubmitError},
    };

    #[kithara::test(native, flash(false))]
    fn owned_pool_failure_returns_payload_and_releases_both_permits() {
        let failed = OnceLock::new();
        assert!(failed.set(Err(String::from("pool build failed"))).is_ok());
        let mut runtime = ComputeRuntime::new(
            WorkerConfig::new()
                .with_owned_pool(OwnedPoolConfig::new(NonZeroUsize::MIN, "failed-pool-test")),
        );
        runtime.pool = ComputePool { owned: failed };
        let task_budget = Arc::new(Budget::default());
        let scope = CancelScope::new(None);
        let token = scope.token().child();
        let rejected = runtime
            .submit(
                &task_budget,
                NonZeroUsize::MIN,
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

    #[kithara::test(native, flash(false))]
    fn worker_budget_reads_the_retained_config_on_each_submission() {
        let pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(2)
                .build()
                .expect("test pool"),
        );
        let mut runtime = ComputeRuntime::new(WorkerConfig::new().with_pool(pool));
        let budget = Arc::new(Budget::default());
        let scope = CancelScope::new(None);
        let token = scope.token().child();
        let (started, started_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        runtime
            .submit(
                &budget,
                NonZeroUsize::MIN,
                &token,
                Wake::default(),
                (),
                move |_, ()| {
                    started.send(()).ok();
                    release_rx.recv().ok();
                },
            )
            .expect("first job admitted");
        started_rx
            .recv_timeout(Instant::now() + Duration::from_secs(2))
            .expect("first job started");

        runtime.config.max_compute_tasks = NonZeroUsize::new(2).expect("nonzero");
        let second_budget = Arc::new(Budget::default());
        let (done, done_rx) = mpsc::channel();
        runtime
            .submit(
                &second_budget,
                NonZeroUsize::MIN,
                &token,
                Wake::default(),
                (),
                move |_, ()| {
                    done.send(()).ok();
                },
            )
            .expect("new config limit admits another job");
        done_rx
            .recv_timeout(Instant::now() + Duration::from_secs(2))
            .expect("second job ran");
        release.send(()).expect("release first job");
    }
}
