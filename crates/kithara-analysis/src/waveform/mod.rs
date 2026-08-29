#[cfg(all(not(target_arch = "wasm32"), feature = "analysis-waveform"))]
mod analyzer;
mod band;
pub(crate) mod bucket;
#[cfg(all(not(target_arch = "wasm32"), feature = "analysis-waveform"))]
mod bucketize;
mod params;

#[cfg(all(not(target_arch = "wasm32"), feature = "analysis-waveform"))]
pub use analyzer::WaveformAnalyzer;
pub(crate) use band::Band;
pub use bucket::Bucket;
pub use params::AnalysisParams;
