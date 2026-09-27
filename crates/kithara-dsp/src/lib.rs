//! Vector DSP kernels over planar `f32` slices.
//!
//! The layout functions match `fast_interleave` for `f32`; the channel-major
//! ones address every plane of one strided slice, so no channel count needs a
//! slice of plane references. The build target picks the backend: Accelerate
//! on Apple, `fearless_simd` at the SIMD level the CPU reports elsewhere.
//! Kernels never allocate, never panic and never sanitize implicitly.
#![forbid(unsafe_code)]
#![deny(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::panic
)]

mod backend;
mod consts;
/// Fade curves: one lexicon for firewheel's `MixDSP` and every crossfade gain.
pub mod fade;
mod layout;
/// Parameter smoothing and A/B mixing owned by firewheel, re-exported as the
/// one import path the workspace uses; a re-export can later become a local
/// type of the same name without touching consumers.
pub mod param;

pub use backend::sanitize;
pub use layout::{deinterleave_channel_major, deinterleave_variable, interleave_channel_major};
