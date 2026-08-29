#[cfg(feature = "analysis-beat")]
use kithara_bufpool::SamplePool;
use kithara_resampler::ResamplerBackend;

use crate::BeatAnalysisConfig;

#[cfg(feature = "analysis-beat")]
pub(crate) fn detector(_sample_pool: &SamplePool) -> Option<Box<dyn crate::beat::BeatDetector>> {
    None
}

pub(crate) fn tag<B>(_config: &BeatAnalysisConfig<B>) -> Option<String>
where
    B: ResamplerBackend,
{
    None
}
