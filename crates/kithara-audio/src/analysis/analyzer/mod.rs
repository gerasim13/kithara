mod config;
mod nn;
mod session;
mod set;
mod snapshot;
mod track;
#[cfg(feature = "analysis-waveform")]
mod waveform;

pub use config::BeatAnalysisConfig;
#[cfg(feature = "analysis-beat")]
pub(crate) use nn::detector as default_beat_detector;
pub(crate) use session::{Ingest, TrackAnalyzers};
pub use set::AnalyzerBuilder;
pub use snapshot::{BeatSnapshot, GridState};
pub use track::{AnalysisFingerprint, AnalysisToken, TrackAnalysis};
#[cfg(feature = "analysis-waveform")]
pub(crate) use waveform::WaveformPass;

pub(crate) use crate::analysis::slots::beat::Detector;
