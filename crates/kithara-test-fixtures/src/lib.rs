#![forbid(unsafe_code)]

//! Build-time generated audio test assets.
//!
//! Asset declarations live in `src/defs/`, compile only into this crate's build
//! script, and never enter the library. The signal primitives they render with
//! do enter it: `signal` is the workspace's one way to make a waveform, a PCM
//! buffer, or a RIFF body, whether at build time or at run time. See
//! `CONTEXT.md` for the store layout and the invalidation contract.

// The store is a host filesystem, and the accessors that read it are generated
// against one. The wasm lane names assets through `SignalAsset` instead and
// fetches their bytes over HTTP, so the store and its accessors stay native.
#[cfg(not(target_arch = "wasm32"))]
pub mod asset;
#[cfg(not(target_arch = "wasm32"))]
pub mod assets;
pub mod signal;
pub mod signal_asset;
// Read by this crate's build script through `#[path]`, and still by the
// integration suite's; declared here so its own tests keep running.
#[cfg(test)]
mod encoders;
#[cfg(not(target_arch = "wasm32"))]
pub mod store;

pub use signal_asset::SignalAsset;
