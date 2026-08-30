#[cfg(not(target_arch = "wasm32"))]
#[path = "compute/native.rs"]
mod platform;
#[cfg(target_arch = "wasm32")]
#[path = "compute/wasm.rs"]
mod platform;

use kithara_platform::{CancelGroup, CancelToken};
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) use platform::ComputePool;
pub(crate) use platform::{Budget, ComputeRuntime};

/// Cancellation context for one admitted compute job.
#[non_exhaustive]
#[derive(Clone)]
pub struct ComputeContext {
    cancel: CancelGroup,
    token: CancelToken,
}

impl ComputeContext {
    /// Derived child token for this compute job.
    #[must_use]
    pub const fn token(&self) -> &CancelToken {
        &self.token
    }

    /// OR-composed task and domain cancellation sources.
    #[must_use]
    pub const fn cancel_group(&self) -> &CancelGroup {
        &self.cancel
    }
}

/// Failure to admit a compute job without queueing it.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ComputeSubmitError {
    /// The owning task or an additional domain cancellation source fired.
    #[error("compute task is cancelled")]
    Cancelled,
    /// This worker has no configured Rayon pool.
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
    payload: T,
    reason: ComputeSubmitError,
}

impl<T> ComputeRejected<T> {
    fn new(reason: ComputeSubmitError, payload: T) -> Self {
        Self { payload, reason }
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
