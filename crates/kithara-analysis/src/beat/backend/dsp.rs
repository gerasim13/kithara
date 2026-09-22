use kithara_beat::{BeatDetectError, SpectralBeats};
use kithara_bufpool::{HasPool, PoolRegion};

use crate::BeatAnalysisConfig;

pub(super) fn detector<B, S>(
    config: &BeatAnalysisConfig<B>,
    pools: &PoolRegion<S>,
) -> Result<SpectralBeats<S>, BeatDetectError>
where
    S: HasPool<f32>,
{
    Ok(SpectralBeats::new(pools.clone(), config.tempo())?)
}
