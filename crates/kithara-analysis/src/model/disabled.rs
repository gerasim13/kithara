#[cfg(feature = "analysis-beat")]
use kithara_bufpool::{HasPool, PoolRegion};
use kithara_resampler::ResamplerBackend;

use crate::BeatAnalysisConfig;

#[cfg(feature = "analysis-beat")]
pub(crate) fn detector<S>(_pools: &PoolRegion<S>) -> Option<Box<dyn crate::beat::BeatDetector>>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    None
}

pub(crate) fn tag<B>(_config: &BeatAnalysisConfig<B>) -> Option<String>
where
    B: ResamplerBackend,
{
    None
}
