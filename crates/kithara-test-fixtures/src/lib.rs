#![forbid(unsafe_code)]

//! Build-time generated audio test assets.
//!
//! Asset declarations live in `src/defs/`, compile only into this crate's build
//! script, and never enter the library. The signal primitives they render with
//! do enter it: `signal` is the workspace's one way to make a waveform, a PCM
//! buffer, or a RIFF body, whether at build time or at run time. See
//! `CONTEXT.md` for the store layout and the invalidation contract.

// Every accessor that reads the store carries its own `cfg`, because the store
// is a host filesystem the browser cannot reach; an `embed` accessor carries
// its bytes instead and compiles everywhere. The store itself stays native, and
// the wasm lane reaches the rest through `SignalAsset` over HTTP.
pub mod asset;
pub mod assets;
pub mod signal;
pub mod signal_asset;
// fMP4 packaging: the build script muxes the packaged bodies it embeds, and the
// integration suite's HLS server muxes its variants on demand. It reads an
// `EncodedTrack`, so it stays off wasm with the encoder that produces one.
#[cfg(not(target_arch = "wasm32"))]
pub mod fmp4;
// Read by this crate's build script through `#[path]`, and still by the
// integration suite's; declared here so its own tests keep running.
#[cfg(test)]
mod encoders;
#[cfg(test)]
mod graph;
#[cfg(not(target_arch = "wasm32"))]
pub mod store;

pub use signal_asset::SignalAsset;
