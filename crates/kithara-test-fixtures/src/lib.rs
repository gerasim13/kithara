#![forbid(unsafe_code)]

//! Build-time generated audio test assets.
//!
//! Asset declarations live in `src/defs/`, compile only into this crate's build
//! script, and never enter the library. The signal primitives they render with
//! do enter it: `signal` is the workspace's one way to make a waveform, a PCM
//! buffer, or a RIFF body, whether at build time or at run time. See
//! `CONTEXT.md` for the store layout and the invalidation contract.

pub mod asset;
pub mod assets;
#[cfg(not(target_arch = "wasm32"))]
pub mod hls;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use hls::manifest as hls_manifest;
#[cfg(test)]
mod context;
#[cfg(test)]
mod encoders;
#[cfg(not(target_arch = "wasm32"))]
pub mod fmp4;
#[cfg(test)]
mod graph;
pub mod signal;
pub mod signal_asset;
#[cfg(not(target_arch = "wasm32"))]
pub mod store;

pub use signal_asset::SignalAsset;
