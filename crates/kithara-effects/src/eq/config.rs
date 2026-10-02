use kithara_bufpool::PoolRegion;
use kithara_config::Config;
use kithara_dsp::param::SmootherConfig;

use crate::consts;

/// Resources shared by one equalizer instance.
#[derive(Config)]
#[config(construction, builder(state_mod(vis = "pub")))]
#[non_exhaustive]
#[derive_where::derive_where(Clone)]
pub struct EqConfig<S> {
    /// Typed pool facade shared with the owning playback region.
    #[config(skip = "injected pool region", builder(start_fn), field(get))]
    pools: PoolRegion<S>,
    /// Runtime gain and layout transition smoothing.
    #[config(
        skip = "consumed by prepared DSP smoothers",
        builder(default = consts::DEFAULT_EQ_SMOOTHING),
        field(get, copy)
    )]
    smoothing: SmootherConfig,
}

impl<S> std::fmt::Debug for EqConfig<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EqConfig")
            .field("pools", &self.pools)
            .field("smoothing", &self.smoothing)
            .finish_non_exhaustive()
    }
}
