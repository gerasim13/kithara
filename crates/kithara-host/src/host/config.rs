use std::{marker::PhantomData, num::NonZeroU32};

use kithara_effects::LimiterConfig;
#[cfg(feature = "offline")]
use {
    kithara_bufpool::PoolRegion,
    kithara_platform::time::Duration,
    kithara_worker::{DispatcherConfig, TaskConfig, WorkerConfig},
};

use crate::HostSettings;

/// Configuration for the shared output session owned by `Host`.
#[cfg_attr(not(feature = "offline"), derive_where::derive_where(Clone, Copy))]
#[non_exhaustive]
pub enum HostConfig<S> {
    /// Device-backed platform session.
    #[non_exhaustive]
    Realtime {
        /// Optional native output callback-size override. `None` preserves the backend default.
        output_block_frames: Option<NonZeroU32>,
        /// Session output limiter policy.
        limiter: LimiterConfig,
        /// Settings the Host starts with; they change while it runs.
        settings: HostSettings,
        marker: PhantomData<fn() -> S>,
    },
    /// Device-free finite renderer.
    #[cfg(feature = "offline")]
    #[non_exhaustive]
    Offline {
        /// Typed output pool shared with the Host's players.
        pools: PoolRegion<S>,
        /// Maximum frames processed by one backend/task quantum.
        max_block_frames: NonZeroU32,
        /// Firewheel smoothing window for graph changes.
        declick_frames: NonZeroU32,
        /// Declared device-equivalent latency used by transport calculations.
        declared_latency: Duration,
        /// Session output limiter policy.
        limiter: LimiterConfig,
        /// Settings the Host starts with; they change while it runs.
        settings: HostSettings,
        /// Shared worker configuration for the session scheduler.
        worker: WorkerConfig,
        /// Dispatcher budgets for the single offline session task.
        dispatcher: Box<DispatcherConfig>,
        /// Admission, priority, and cancellation configuration for the session task.
        task: TaskConfig,
    },
}

#[bon::bon]
impl<S> HostConfig<S> {
    /// Configure a platform realtime session.
    #[builder(
        builder_type(vis = "pub"),
        state_mod(vis = "pub"),
        start_fn(name = builder, vis = "pub")
    )]
    fn new(
        output_block_frames: Option<NonZeroU32>,
        #[builder(default)] limiter: LimiterConfig,
        #[builder(default)] settings: HostSettings,
    ) -> Self {
        Self::Realtime {
            output_block_frames,
            limiter,
            settings,
            marker: PhantomData,
        }
    }

    /// Settings the Host starts with.
    #[must_use]
    pub const fn settings(&self) -> HostSettings {
        match self {
            Self::Realtime { settings, .. } => *settings,
            #[cfg(feature = "offline")]
            Self::Offline { settings, .. } => *settings,
        }
    }
}
