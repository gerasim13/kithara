use std::num::{NonZeroU32, NonZeroUsize};

use bon::Builder;
use kithara_bufpool::{BytePool, SamplePool};
use kithara_platform::{CancelToken, time::Duration};
use kithara_worker::Worker;

struct Consts;

impl Consts {
    const CAPACITY: NonZeroUsize = match NonZeroUsize::new(16) {
        Some(value) => value,
        None => unreachable!(),
    };
    const FAIRNESS_YIELD_INTERVAL: NonZeroU32 = match NonZeroU32::new(16) {
        Some(value) => value,
        None => unreachable!(),
    };
    const TASK_BURST: NonZeroU32 = match NonZeroU32::new(32) {
        Some(value) => value,
        None => unreachable!(),
    };
}

/// Configuration for one shared playback worker.
#[derive(Builder, fieldwork::Fieldwork)]
#[builder(start_fn = for_pools)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct PlayWorkerConfig {
    /// Byte pool shared by every Player and resource registered with the worker.
    #[builder(start_fn)]
    #[field(get)]
    pub(crate) byte_pool: BytePool,
    /// Sample pool shared by every Player and resource registered with the worker.
    #[builder(start_fn)]
    #[field(get)]
    pub(crate) sample_pool: SamplePool,
    /// Park duration when no playback task expects progress.
    #[builder(default = Duration::from_millis(100))]
    #[field(get, copy)]
    pub(crate) idle_timeout: Duration,
    /// Threshold for reporting a slow playback tick.
    #[builder(default = Duration::from_millis(10))]
    #[field(get, copy)]
    pub(crate) slow_tick_threshold: Duration,
    /// Park duration while live playback tasks are waiting.
    #[builder(default = Duration::from_millis(10))]
    #[field(get, copy)]
    pub(crate) wait_timeout: Duration,
    /// Consecutive progress passes between cooperative thread yields.
    #[builder(default = Consts::FAIRNESS_YIELD_INTERVAL)]
    #[field(get, copy)]
    pub(crate) fairness_yield_interval: NonZeroU32,
    /// Maximum consecutive ticks for one track visit.
    #[builder(default = Consts::TASK_BURST)]
    #[field(get, copy)]
    pub(crate) task_burst: NonZeroU32,
    /// Maximum number of simultaneously registered track render chains.
    #[builder(default = Consts::CAPACITY)]
    #[field(get, copy)]
    pub(crate) capacity: NonZeroUsize,
    /// Parent cancellation token for this playback dispatcher lifetime.
    pub(crate) cancel: Option<CancelToken>,
    /// Optional base worker shared with other domain workers.
    pub(crate) worker: Option<Worker>,
}
