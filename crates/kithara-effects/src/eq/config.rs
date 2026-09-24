use bon::Builder;
use firewheel::{dsp::filter::smoothing_filter::MIN_SETTLE_RATIO, param::smoother::SmootherConfig};
use kithara_bufpool::PoolRegion;

const DEFAULT_EQ_SMOOTHING: SmootherConfig = SmootherConfig {
    smooth_seconds: 0.01,
    // A band settles only once it is within this fraction of the gain span.
    // The tightest the smoother accepts: a band that snaps early is a step in
    // the frequency response, which is exactly what the smoothing is here to
    // keep out.
    settle_ratio: MIN_SETTLE_RATIO,
};

/// Resources shared by one equalizer instance.
#[derive(Builder, fieldwork::Fieldwork)]
#[builder(state_mod(vis = "pub"))]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
#[derive_where::derive_where(Clone)]
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

impl<S> std::fmt::Debug for EqConfig<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EqConfig")
            .field("pools", &self.pools)
            .field("smoothing", &self.smoothing)
            .finish_non_exhaustive()
    }
}
