use kithara_bufpool::{HasPool, PoolRegion};
use kithara_platform::sync::Arc;

use crate::BeatAnalysisConfig;

pub(crate) fn detector<B, S>(
    _config: &BeatAnalysisConfig<B>,
    _pools: &PoolRegion<S>,
) -> Option<Arc<dyn crate::beat::BeatDetector>>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    None
}
