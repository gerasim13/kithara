mod beat;
mod snapshot;
mod track;

pub use beat::BeatArtifact;
#[cfg(any(test, feature = "analysis-beat"))]
pub(crate) use beat::FitRegion;
#[cfg(feature = "analysis-beat")]
pub(crate) use beat::MarkedBeat;
pub use snapshot::{BeatSnapshot, BeatState};
pub use track::{AnalysisFingerprint, AnalysisToken, TrackAnalysis};
