use bon::bon;
use thiserror::Error;

use crate::{
    config::BeatConfig, inference::BeatPredictor, mel::MelExtractor, postprocess::PeakPicker,
};

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BeatError {
    #[error("model load failed ({model}): {reason}")]
    ModelLoad { model: &'static str, reason: String },
    #[error("inference failed: {reason}")]
    Inference { reason: String },
}

/// One detected beat or downbeat: where it is, and how sure the model was.
/// Paired rather than kept in parallel vectors, which stages above would only
/// have to keep in step.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct BeatMark {
    /// Seconds from the start of the analysed audio.
    pub at: f32,
    /// Probability the model assigned this peak, in `(0, 1)`.
    pub confidence: f32,
}

/// Beat / downbeat marks in seconds, whole-track.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RawBeats {
    pub beats: Vec<BeatMark>,
    pub downbeats: Vec<BeatMark>,
}

/// `beat_this` NN detector: mel → chunked inference → peak picking.
pub struct BeatThis {
    predictor: BeatPredictor,
    mel: MelExtractor,
    picker: PeakPicker,
}

#[bon]
impl BeatThis {
    /// Models from mel and beat ONNX bytes, decoded with `config`.
    ///
    /// # Errors
    /// [`BeatError::ModelLoad`] when either model fails to parse.
    #[builder]
    pub fn new(
        mel_model: &[u8],
        beat_model: &[u8],
        #[builder(default)] config: BeatConfig,
    ) -> Result<Self, BeatError> {
        Ok(Self {
            mel: MelExtractor::try_from(mel_model)?,
            predictor: BeatPredictor::try_from(beat_model)?,
            picker: PeakPicker::new(config),
        })
    }

    /// Input: whole-track mono f32 at `22_050` Hz. Output: seconds.
    ///
    /// # Errors
    /// [`BeatError::Inference`] when a model run fails or emits an
    /// unexpected output shape.
    pub fn analyze(&mut self, mono_22050: &[f32]) -> Result<RawBeats, BeatError> {
        let mel = self.mel.extract(mono_22050)?;
        let (beat_logits, downbeat_logits) = self.predictor.predict(&mel)?;
        let (beats, downbeats) = self.picker.decode(&beat_logits, &downbeat_logits)?;
        Ok(RawBeats { beats, downbeats })
    }
}
