use kithara_beat::{BeatDetectError, BeatDetector};
use kithara_bufpool::{HasPool, PoolRegion};

use super::BeatDetectorKind;
use crate::BeatAnalysisConfig;

pub(crate) fn build_detector<B, S>(
    kind: BeatDetectorKind,
    config: &BeatAnalysisConfig<B>,
    pools: &PoolRegion<S>,
) -> Result<Box<dyn BeatDetector>, BeatDetectError>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    match kind {
        #[cfg(feature = "beat-nn")]
        BeatDetectorKind::NnBeatThis => Ok(Box::new(super::nn::detector(config, pools)?)),
        #[cfg(feature = "beat-dsp")]
        BeatDetectorKind::DspSpectral => Ok(Box::new(super::dsp::detector(config, pools)?)),
    }
}
