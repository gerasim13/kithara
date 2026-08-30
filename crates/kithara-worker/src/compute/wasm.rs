use std::num::NonZeroUsize;

use kithara_platform::{CancelGroup, CancelToken, sync::Arc};

use super::{ComputeContext, ComputeRejected, ComputeSubmitError};
use crate::{Wake, config::PoolConfig};

pub(crate) struct ComputeRuntime;

impl ComputeRuntime {
    pub(crate) fn new(pool: PoolConfig, max_in_flight: NonZeroUsize) -> Self {
        let _ = (pool, max_in_flight);
        Self
    }

    pub(crate) fn submit<T, F>(
        &self,
        task_budget: &Arc<Budget>,
        task_token: &CancelToken,
        task_cancel: &CancelGroup,
        wake: Wake,
        payload: T,
        job: F,
    ) -> Result<(), ComputeRejected<T>>
    where
        T: Send + 'static,
        F: FnOnce(ComputeContext, T) + Send + 'static,
    {
        let _ = (task_budget, task_token, wake, job);
        let reason = if task_cancel.is_cancelled() {
            ComputeSubmitError::Cancelled
        } else {
            ComputeSubmitError::Unavailable
        };
        Err(ComputeRejected::new(reason, payload))
    }
}

pub(crate) struct Budget;

impl Budget {
    pub(crate) const fn new(limit: NonZeroUsize) -> Self {
        let _ = limit;
        Self
    }
}
