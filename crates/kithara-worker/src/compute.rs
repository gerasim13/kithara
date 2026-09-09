#[cfg(not(target_arch = "wasm32"))]
#[path = "compute/native.rs"]
mod platform;
#[cfg(target_arch = "wasm32")]
#[path = "compute/wasm.rs"]
mod platform;

use std::num::NonZeroUsize;

use kithara_platform::{
    CancelGroup, CancelToken,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
pub(crate) use platform::ComputePool;

use crate::{Wake, config::PoolConfig};

/// Worker-owned compute seam pairing budget admission with a platform pool.
pub(crate) struct ComputeRuntime {
    budget: Arc<Budget>,
    pool: ComputePool,
}

impl ComputeRuntime {
    pub(crate) fn new(pool: PoolConfig, max_in_flight: NonZeroUsize) -> Self {
        Self {
            budget: Arc::new(Budget::new(max_in_flight)),
            pool: ComputePool::new(pool),
        }
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) const fn pool(&self) -> &ComputePool {
        &self.pool
    }

    pub(crate) fn submit<T, F>(
        &self,
        task_budget: &Arc<Budget>,
        task_token: &CancelToken,
        wake: Wake,
        payload: T,
        job: F,
    ) -> Result<(), ComputeRejected<T>>
    where
        T: Send + 'static,
        F: FnOnce(ComputeContext, T) + Send + 'static,
    {
        if task_token.is_cancelled() {
            return Err(ComputeRejected::new(ComputeSubmitError::Cancelled, payload));
        }
        if self.pool.is_disabled() {
            return Err(ComputeRejected::new(
                ComputeSubmitError::Unavailable,
                payload,
            ));
        }
        let Some(task_permit) = Budget::try_acquire(task_budget) else {
            return Err(ComputeRejected::new(ComputeSubmitError::Saturated, payload));
        };
        let Some(worker_permit) = Budget::try_acquire(&self.budget) else {
            return Err(ComputeRejected::new(ComputeSubmitError::Saturated, payload));
        };
        if task_token.is_cancelled() {
            return Err(ComputeRejected::new(ComputeSubmitError::Cancelled, payload));
        }
        let spawner = match self.pool.spawner() {
            Ok(spawner) => spawner,
            Err(reason) => return Err(ComputeRejected::new(reason, payload)),
        };
        if task_token.is_cancelled() {
            return Err(ComputeRejected::new(ComputeSubmitError::Cancelled, payload));
        }
        let token = task_token.child();
        let context = ComputeContext {
            cancel: CancelGroup::from(token.clone()),
            token,
        };
        let permit = ComputePermit {
            wake,
            task: Some(task_permit),
            worker: Some(worker_permit),
        };

        spawner.spawn(move || {
            let _permit = permit;
            job(context, payload);
        });
        Ok(())
    }
}

/// In-flight admission counter.
pub(crate) struct Budget {
    active: AtomicUsize,
    limit: NonZeroUsize,
}

impl Budget {
    pub(crate) fn new(limit: NonZeroUsize) -> Self {
        Self {
            limit,
            active: AtomicUsize::new(0),
        }
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    fn try_acquire(budget: &Arc<Self>) -> Option<BudgetPermit> {
        let mut active = budget.active.load(Ordering::Acquire);
        while active < budget.limit.get() {
            match budget.active.compare_exchange_weak(
                active,
                active + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(BudgetPermit {
                        budget: Arc::clone(budget),
                    });
                }
                Err(current) => active = current,
            }
        }
        None
    }
}

struct BudgetPermit {
    budget: Arc<Budget>,
}

impl Drop for BudgetPermit {
    fn drop(&mut self) {
        self.budget.active.fetch_sub(1, Ordering::AcqRel);
    }
}

struct ComputePermit {
    task: Option<BudgetPermit>,
    worker: Option<BudgetPermit>,
    wake: Wake,
}

impl Drop for ComputePermit {
    fn drop(&mut self) {
        drop(self.task.take());
        drop(self.worker.take());
        self.wake.wake();
    }
}

/// Cancellation context for one admitted compute job.
#[non_exhaustive]
#[derive(Clone)]
pub struct ComputeContext {
    cancel: CancelGroup,
    token: CancelToken,
}

impl ComputeContext {
    /// Cancellation group containing only this compute job's derived token.
    #[must_use]
    pub const fn cancel_group(&self) -> &CancelGroup {
        &self.cancel
    }

    /// Derived child token for this compute job.
    #[must_use]
    pub const fn token(&self) -> &CancelToken {
        &self.token
    }
}

/// Failure to admit a compute job without queueing it.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ComputeSubmitError {
    /// The owning task or an additional domain cancellation source fired.
    #[error("compute task is cancelled")]
    Cancelled,
    /// This worker has no configured compute pool.
    #[error("compute pool is unavailable")]
    Unavailable,
    /// The task or worker in-flight budget is exhausted.
    #[error("compute budget is saturated")]
    Saturated,
}

/// Rejected compute submission retaining ownership of its payload.
#[non_exhaustive]
#[derive(Debug)]
pub struct ComputeRejected<T> {
    reason: ComputeSubmitError,
    payload: T,
}

impl<T> ComputeRejected<T> {
    fn new(reason: ComputeSubmitError, payload: T) -> Self {
        Self { reason, payload }
    }

    /// Return why the compute job was rejected.
    #[must_use]
    pub const fn reason(&self) -> ComputeSubmitError {
        self.reason
    }

    /// Recover the payload for retry or domain-owned cleanup.
    #[must_use]
    pub fn recover_payload(self) -> T {
        self.payload
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use kithara_platform::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc::{self, TryRecvError},
        },
        thread::current_thread_id,
        time::{self, Duration, Instant},
    };
    use kithara_test_utils::kithara;

    use crate::{
        DispatcherConfig, OwnedPoolConfig, Task, TaskConfig, TaskContext, TickResult, Worker,
        WorkerConfig,
    };

    #[derive(Debug, Eq, PartialEq)]
    enum Step {
        Admitted { dispatcher: u64, ok: bool },
        Ran { compute: u64 },
        Woken,
    }

    struct ThreadProbe {
        context: TaskContext,
        steps: mpsc::Sender<Step>,
        ran: Arc<AtomicBool>,
        submitted: bool,
    }

    impl Task for ThreadProbe {
        fn tick(&mut self) -> TickResult {
            if self.submitted {
                if !self.ran.load(Ordering::Acquire) {
                    return TickResult::Waiting;
                }
                self.steps.send(Step::Woken).ok();
                return TickResult::Done;
            }
            self.submitted = true;
            let steps = self.steps.clone();
            let ran = Arc::clone(&self.ran);
            let admitted = self.context.submit_compute((), move |_, ()| {
                steps
                    .send(Step::Ran {
                        compute: current_thread_id(),
                    })
                    .ok();
                ran.store(true, Ordering::Release);
            });
            self.steps
                .send(Step::Admitted {
                    dispatcher: current_thread_id(),
                    ok: admitted.is_ok(),
                })
                .ok();
            TickResult::Waiting
        }
    }

    async fn next_step(observed: &mpsc::Receiver<Step>, deadline: Instant, what: &str) -> Step {
        loop {
            match observed.try_recv() {
                Ok(step) => return step,
                Err(TryRecvError::Empty) => {
                    assert!(
                        Instant::now() < deadline,
                        "the dispatcher must report {what}"
                    );
                    time::sleep(Duration::from_millis(5)).await;
                }
                Err(closed) => panic!("the probe stopped reporting {what}: {closed:?}"),
            }
        }
    }

    #[kithara::test(tokio, browser, flash(false))]
    async fn compute_job_runs_off_the_dispatcher_thread_and_wakes_it() {
        let worker = Worker::new(WorkerConfig::new().with_owned_pool(OwnedPoolConfig::new(
            NonZeroUsize::MIN,
            "compute-thread-test",
        )));
        let dispatcher = worker.dispatcher(
            DispatcherConfig::builder()
                .name("compute-thread-test")
                .idle_timeout(Duration::from_secs(2))
                .wait_timeout(Duration::from_secs(2))
                .build(),
        );
        let (steps, observed) = mpsc::channel();
        let _handle = dispatcher
            .register(TaskConfig::new(), move |context| ThreadProbe {
                context,
                steps,
                ran: Arc::new(AtomicBool::new(false)),
                submitted: false,
            })
            .expect("probe task admission");

        let run_by = Instant::now() + Duration::from_secs(10);
        let mut recorded = Vec::new();
        while recorded.len() < 2 {
            recorded.push(next_step(&observed, run_by, "admission and the run").await);
        }
        let woken_by = Instant::now() + Duration::from_secs(1);
        recorded.push(next_step(&observed, woken_by, "the wake that follows the run").await);

        let Some(&Step::Admitted { dispatcher, ok }) = recorded
            .iter()
            .find(|step| matches!(step, Step::Admitted { .. }))
        else {
            panic!("compute submission was never reported: {recorded:?}");
        };
        let Some(&Step::Ran { compute }) = recorded
            .iter()
            .find(|step| matches!(step, Step::Ran { .. }))
        else {
            panic!("compute job never ran: {recorded:?}");
        };
        assert!(ok, "compute admission must succeed");
        assert_eq!(
            recorded.last(),
            Some(&Step::Woken),
            "completion must wake the dispatcher last"
        );
        assert_ne!(
            dispatcher, compute,
            "compute job must not run on the dispatcher thread"
        );
    }
}
