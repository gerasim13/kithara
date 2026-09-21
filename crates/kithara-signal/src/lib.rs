#![deny(unsafe_code)]

//! Decoded-audio signal values and pure sample/time math.

mod chunk;
mod coverage;
mod error;
mod fader;
mod interleaved;
mod planar;
mod sample;
mod session;
mod spec;
#[cfg(test)]
pub(crate) use kithara_test_utils::bufpool as test_pools;
mod time;
mod units;

pub use chunk::{AudioChunk, AudioChunkInfo};
pub use coverage::{CoverageRead, CoverageWrite, FrameCoverage, FrameSpan};
pub use error::SignalError;
pub use fader::FaderValue;
pub use interleaved::InterleavedView;
pub use planar::{PlanarBuffer, PlanarView};
pub use sample::sanitize_sample;
pub use session::{OutputContext, SessionEpoch, SessionFrame, TransportRevision};
pub use spec::AudioSpec;
pub use units::{FrameCount, SampleCount};
