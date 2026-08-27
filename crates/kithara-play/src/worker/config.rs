use std::num::NonZeroUsize;

use bon::Builder;
use kithara_bufpool::{BytePool, PcmPool};
use kithara_platform::CancelToken;

const DEFAULT_CAPACITY: NonZeroUsize = match NonZeroUsize::new(16) {
    Some(capacity) => capacity,
    None => unreachable!(),
};

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
    /// PCM pool shared by every Player and resource registered with the worker.
    #[builder(start_fn)]
    #[field(get)]
    pub(crate) pcm_pool: PcmPool,
    /// Parent cancellation token for the worker lifetime.
    pub(crate) cancel: Option<CancelToken>,
    /// Maximum number of simultaneously registered track render chains.
    #[builder(default = DEFAULT_CAPACITY)]
    #[field(get, copy)]
    pub(crate) capacity: NonZeroUsize,
}
