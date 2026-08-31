use bon::Builder;
use kithara_bufpool::PoolRegion;

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
}

impl<S> Clone for EqConfig<S> {
    fn clone(&self) -> Self {
        Self {
            pools: self.pools.clone(),
        }
    }
}

impl<S> std::fmt::Debug for EqConfig<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EqConfig")
            .field("pools", &self.pools)
            .finish_non_exhaustive()
    }
}
