use bon::Builder;
use kithara_bufpool::SamplePool;

/// Resources shared by one equalizer instance.
#[derive(Clone, Debug, Builder, fieldwork::Fieldwork)]
#[builder(start_fn = for_pool, state_mod(vis = "pub"))]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct EqConfig {
    /// Sample pool shared with the owning playback region.
    #[builder(start_fn)]
    #[field(get)]
    sample_pool: SamplePool,
}
