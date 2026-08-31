//! Progressive source-signal analysis and reusable analysis artifacts.

#![forbid(unsafe_code)]

#[cfg(not(target_arch = "wasm32"))]
mod analyzer;
mod archive;
mod artifact;
#[cfg(all(not(target_arch = "wasm32"), feature = "analysis-beat"))]
pub(crate) mod beat;
mod blob;
mod coverage;
#[cfg(not(target_arch = "wasm32"))]
mod model;
#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod producer;
mod progress;
#[cfg(not(target_arch = "wasm32"))]
mod slots;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
mod waveform;
#[cfg(not(target_arch = "wasm32"))]
mod worker;

pub use archive::{
    AnalysisFile, AnalysisFileError, AnalysisFilePatch, AnalysisFileSpec, AnalysisFileUpdate,
    AnalysisFileWrite,
};
pub use artifact::{
    AnalysisFingerprint, AnalysisToken, BeatArtifact, BeatSnapshot, BeatState, TrackAnalysis,
};
pub use blob::frame::BlobError;
pub use coverage::{Coverage, FrameRange};
#[cfg(not(target_arch = "wasm32"))]
pub use native::*;
pub use progress::AnalysisProgress;
pub use waveform::{AnalysisParams, Bucket, bucket::Waveform};
