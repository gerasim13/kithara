#[cfg(feature = "resample-glide")]
pub use kithara_resampler::glide::{GlideBackend, GlideConfig, GlideInterpolation};
#[cfg(feature = "resample-rubato")]
pub use kithara_resampler::rubato::{RubatoAlgorithm, RubatoBackend, RubatoConfig};

#[cfg(not(target_arch = "wasm32"))]
pub use crate::effects::timestretch::{
    ElasticEngine, ElasticError, StretchKind, TimeStretchProcessor,
};
#[cfg(feature = "analysis-waveform")]
pub use crate::waveform::WaveformAnalyzer;
