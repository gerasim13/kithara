use kithara_beat::{BEAT_MODEL_BYTES, BeatThis, MEL_MODEL_BYTES};
use kithara_bufpool::{HasPool, PoolRegion};

use super::{BeatDetectError, BeatDetector, BeatMark, RawBeats};

#[derive(Debug, Clone, Copy, derive_more::Display, PartialEq, Eq)]
#[display("{self:?}")]
pub(crate) enum BeatDetectorKind {
    NnBeatThis,
}

impl BeatDetectorKind {
    pub(crate) const ALL: &'static [Self] = &[Self::NnBeatThis];

    pub(crate) fn first() -> Self {
        Self::ALL[0]
    }
}

impl Default for BeatDetectorKind {
    fn default() -> Self {
        Self::first()
    }
}

pub(crate) fn build_detector<S>(
    kind: BeatDetectorKind,
    pools: &PoolRegion<S>,
) -> Result<Box<dyn BeatDetector>, BeatDetectError>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    match kind {
        BeatDetectorKind::NnBeatThis => Ok(Box::new(NnDetector::new(pools)?)),
    }
}

struct NnDetector<S>
where
    S: HasPool<f32>,
{
    inner: BeatThis<S>,
}

impl<S> NnDetector<S>
where
    S: HasPool<f32>,
{
    fn new(pools: &PoolRegion<S>) -> Result<Self, BeatDetectError> {
        let inner = BeatThis::builder()
            .mel_model(MEL_MODEL_BYTES)
            .beat_model(BEAT_MODEL_BYTES)
            .pools(pools.clone())
            .build()
            .map_err(|e| BeatDetectError::Init {
                reason: e.to_string(),
            })?;
        Ok(Self { inner })
    }
}

impl<S> BeatDetector for NnDetector<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    fn detect(&self, mono_window: &[f32]) -> Result<RawBeats, BeatDetectError> {
        let raw = self
            .inner
            .analyze(mono_window)
            .map_err(|e| BeatDetectError::Detect {
                reason: e.to_string(),
            })?;
        Ok(RawBeats {
            beats: raw.beats.into_iter().map(mark).collect(),
            downbeats: raw.downbeats.into_iter().map(mark).collect(),
        })
    }
}

fn mark(mark: kithara_beat::BeatMark) -> BeatMark {
    BeatMark {
        at: mark.at,
        confidence: mark.confidence,
    }
}
