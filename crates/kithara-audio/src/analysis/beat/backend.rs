use kithara_beat::{BEAT_MODEL_BYTES, BeatThis, MEL_MODEL_BYTES};

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

pub(crate) fn build_detector(
    kind: BeatDetectorKind,
) -> Result<Box<dyn BeatDetector>, BeatDetectError> {
    match kind {
        BeatDetectorKind::NnBeatThis => Ok(Box::new(NnDetector::new()?)),
    }
}

struct NnDetector {
    inner: BeatThis,
}

impl NnDetector {
    fn new() -> Result<Self, BeatDetectError> {
        let inner = BeatThis::builder()
            .mel_model(MEL_MODEL_BYTES)
            .beat_model(BEAT_MODEL_BYTES)
            .build()
            .map_err(|e| BeatDetectError::Init {
                reason: e.to_string(),
            })?;
        Ok(Self { inner })
    }
}

impl BeatDetector for NnDetector {
    fn detect(&mut self, mono_window: &[f32]) -> Result<RawBeats, BeatDetectError> {
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
