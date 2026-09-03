mod beat;
mod grid;
mod snapshot;
mod track;

pub use beat::BeatArtifact;
#[cfg(any(test, all(not(target_arch = "wasm32"), feature = "analysis-beat")))]
pub(crate) use beat::FitRegion;
#[cfg(all(not(target_arch = "wasm32"), feature = "analysis-beat"))]
pub(crate) use beat::MarkedBeat;
pub use grid::{BeatGridConfig, BeatGridError};
pub use snapshot::{BeatSnapshot, BeatState};
pub use track::{AnalysisFingerprint, AnalysisToken, TrackAnalysis};
