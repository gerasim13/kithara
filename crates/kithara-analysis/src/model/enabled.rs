use kithara_bufpool::SamplePool;
use kithara_resampler::ResamplerBackend;
use tracing::warn;

use crate::{
    BeatAnalysisConfig,
    beat::{BeatDetectorKind, GRID_SEMANTICS_TAG, GridParams, build_detector},
};

const NN_MODEL_TAG: &str = "beat_this_small_v1";

pub(crate) fn detector(sample_pool: &SamplePool) -> Option<Box<dyn crate::beat::BeatDetector>> {
    match build_detector(BeatDetectorKind::default(), sample_pool) {
        Ok(detector) => Some(detector),
        Err(e) => {
            warn!(?e, "beat detector init failed; beat analysis disabled");
            None
        }
    }
}

pub(crate) fn tag<B>(config: &BeatAnalysisConfig<B>) -> Option<String>
where
    B: ResamplerBackend,
{
    BeatDetectorKind::ALL.first().map(|kind| {
        format!(
            "{kind}:{NN_MODEL_TAG}:{}:{:?}:{:?}",
            GRID_SEMANTICS_TAG,
            GridParams::default(),
            config
        )
    })
}
