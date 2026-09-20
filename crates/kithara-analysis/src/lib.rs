//! Progressive source-signal analysis and reusable analysis artifacts.

#![forbid(unsafe_code)]

mod analyzer;
mod archive;
mod artifact;
#[cfg(feature = "analysis-beat")]
pub(crate) mod beat;
mod blob;
mod coverage;
mod model;
pub(crate) mod producer;
mod progress;
mod slots;
#[cfg(test)]
pub(crate) use kithara_test_utils::bufpool as test_pools;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
mod waveform;
mod worker;

pub use analyzer::{
    AnalyzerBuilder, BeatAnalysisConfig, BeatAnalysisConfigPatch, BeatAnalysisConfigPatchError,
};
pub use archive::{
    AnalysisFile, AnalysisFileError, AnalysisFilePatch, AnalysisFileSpec, AnalysisFileUpdate,
    AnalysisFileWrite,
};
pub use artifact::{
    AnalysisFingerprint, AnalysisToken, BeatArtifact, BeatGridUnavailable, BeatSnapshot, BeatState,
    ORDINAL_TOLERANCE_BEATS, TrackAnalysis,
};
pub use blob::frame::BlobError;
pub use coverage::{Coverage, FrameRange};
/// The served beat-grid contract, re-exported from its owner so a consumer of
/// a publication can name what [`TrackAnalysis::grid`] hands it. The types are
/// `kithara-beat`'s own: a server reading a stored grid needs no analyzer.
pub use kithara_beat::{
    BeatGridError, BeatGridModel, BeatGridState, GridBeat, GridDownbeat, Meter, RawBeatGrid,
};
pub use producer::AnalysisProducer;
pub use progress::AnalysisProgress;
#[cfg(feature = "analysis-waveform")]
pub use waveform::WaveformAnalyzer;
pub use waveform::{AnalysisParams, Bucket, bucket::Waveform};
pub use worker::{AnalysisOpen, AnalysisPass, AnalysisWorker, AnalysisWorkerConfig};
