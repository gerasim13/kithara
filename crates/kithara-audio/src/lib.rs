//! Audio pipeline library with decoding and resampling.
//!
//! - [`Audio`] — generic PCM reader prepared for an external playback scheduler
//! - [`AudioConfig`] — pipeline configuration
//! - [`ResamplerQuality`] - sample rate conversion quality
//! - `Audio` implements [`PcmReader`] for pull-based PCM consumers
//!
//! See the crate `README.md` for usage and `CONTEXT.md` for threading model and architecture.

#![forbid(unsafe_code)]
#![cfg_attr(all(rtsan, not(rtsan_standalone)), feature(sanitize))]

pub mod analysis;
mod audio;
mod blob;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
mod pipeline;
mod producer;
mod runtime;
mod traits;
mod waveform;

pub use audio::{Audio, PreparedAudio, SeekHandle};
pub use blob::frame::BlobError;
#[cfg(feature = "resample-glide")]
pub use kithara_resampler::glide::{GlideBackend, GlideConfig, GlideInterpolation};
#[cfg(feature = "resample-rubato")]
pub use kithara_resampler::rubato::{RubatoAlgorithm, RubatoBackend, RubatoConfig};
pub use kithara_resampler::{
    NoResamplerBackend, ResamplerBackend, ResamplerOptions, ResamplerQuality,
};
pub use pipeline::{
    config::{AudioConfig, AudioDecoderConfig, ConsumerWakeMode, DecoderResamplerSettings},
    fetch::{EpochValidator, Fetch, SourceEnd},
    track::{TrackStep, WaitingReason},
};
pub use producer::PreloadGate;
#[doc(hidden)]
pub use producer::{PcmProducerPort, PreparedPcmLane};
pub use traits::{
    ChunkOutcome, DecodeError, DecodeResult, PcmControl, PcmObserveError, PcmObserver, PcmRead,
    PcmReader, PcmSession, PcmSource, PendingReason, ReadOutcome, SeekBegin, SeekOutcome,
    SourceDiscontinuity,
};
#[cfg(feature = "analysis-waveform")]
pub use waveform::WaveformAnalyzer;
pub use waveform::{AnalysisParams, BeatGrid, Bucket, bucket::Waveform};
