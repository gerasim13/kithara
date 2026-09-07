use bon::Builder;
use firewheel::param::smoother::SmootherConfig;
use kithara_bufpool::PoolRegion;

const DEFAULT_EQ_SMOOTHING: SmootherConfig = SmootherConfig {
    smooth_seconds: 0.01,
    settle_epsilon: 0.0001,
};

/// Resources shared by one equalizer instance.
#[derive(Builder, fieldwork::Fieldwork)]
#[builder(state_mod(vis = "pub"))]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct EqConfig<S> {
    /// Typed pool facade shared with the owning playback region.
    #[builder(start_fn)]
    #[field(get)]
    pools: PoolRegion<S>,
    /// Runtime gain and layout transition smoothing.
    #[builder(default = DEFAULT_EQ_SMOOTHING)]
    #[field(get, copy)]
    smoothing: SmootherConfig,
}

impl<S> Clone for EqConfig<S> {
    fn clone(&self) -> Self {
        Self {
            pools: self.pools.clone(),
            smoothing: self.smoothing,
        }
    }
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
