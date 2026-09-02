use std::{fmt, num::NonZeroU32};

use bon::Builder;
use kithara_bufpool::PoolRegion;
use kithara_platform::{CancelToken, sync::Arc};
use kithara_warp::BeatGridId;

use crate::{
    effects::eq::{EqBandConfig, generate_log_spaced_bands},
    session::SessionDispatcher,
};

/// Configuration for the audio engine.
#[derive(Builder)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct EngineConfig<S> {
    /// Stable synchronization identity of the owning player.
    pub(crate) grid_id: BeatGridId,
    /// Master cancel token for the engine. The worker scheduler derives a
    /// `child()` so its produce-core's lock-free `is_cancelled()` read
    /// observes a master cancel.
    pub(crate) cancel: Option<CancelToken>,
    /// Optional pre-bound dispatcher for isolated harnesses. Production
    /// engines receive their session when the owning Player enters a Host.
    pub(crate) session: Option<Arc<dyn SessionDispatcher<S>>>,
    /// Typed pool facade for audio-thread scratch buffers.
    pub(crate) pools: PoolRegion<S>,
    /// EQ band layout per player. Default: 10-band log-spaced.
    #[builder(default = generate_log_spaced_bands(10))]
    pub(crate) eq_layout: Vec<EqBandConfig>,
    /// Number of output channels. Default: 2 (stereo).
    #[builder(default = 2)]
    pub(crate) channels: u16,
    /// Initial output sample rate supplied by the owning player session.
    pub(crate) sample_rate: NonZeroU32,
    /// Maximum number of concurrent player slots. Default: 4.
    #[builder(default = 4)]
    pub(crate) max_slots: usize,
}

impl<S> Clone for EngineConfig<S> {
    fn clone(&self) -> Self {
        Self {
            grid_id: self.grid_id,
            cancel: self.cancel.clone(),
            session: self.session.clone(),
            pools: self.pools.clone(),
            eq_layout: self.eq_layout.clone(),
            channels: self.channels,
            sample_rate: self.sample_rate,
            max_slots: self.max_slots,
        }
    }
}

impl<S> fmt::Debug for EngineConfig<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineConfig")
            .field("eq_layout", &self.eq_layout)
            .field("channels", &self.channels)
            .field("sample_rate", &self.sample_rate)
            .field("max_slots", &self.max_slots)
            .field("pools", &self.pools)
            .finish_non_exhaustive()
    }
}
