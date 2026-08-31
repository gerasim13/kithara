mod error;
mod file;
mod update;

#[cfg(all(
    test,
    not(target_arch = "wasm32"),
    feature = "analysis-beat",
    feature = "analysis-waveform"
))]
mod resume_tests;
#[cfg(test)]
mod tests;

pub use error::AnalysisFileError;
pub use file::{AnalysisFile, AnalysisFileSpec};
pub use update::{AnalysisFilePatch, AnalysisFileUpdate, AnalysisFileWrite};
