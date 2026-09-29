use kithara_bufpool::PoolError;
use kithara_dsp::spectrum::SpectrumError;

/// Why a waveform analyzer could not be built.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AnalyzerError {
    /// No backend runs the FFT at the configured length.
    #[error("waveform analysis FFT could not be built: {0}")]
    Spectrum(#[from] SpectrumError),
    /// The spectrum the analyzer reduces does not fit the region budget.
    #[error("waveform analysis buffer allocation failed: {0}")]
    Pool(#[from] PoolError),
}
