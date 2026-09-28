#![forbid(unsafe_code)]

//! The media both halves of the fixture pipeline agree on: the synthetic
//! signals a fixture is rendered from, the fMP4 and HLS shapes it is packaged
//! in, and the content-addressed store it is kept in. The build that produces
//! fixtures and the accessors that read them compile these same types.

/// The gapless request shape is portable; the muxer is native-only.
pub mod fmp4;
#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
pub mod hls_manifest;
pub mod signal;
#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
pub mod store;
#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
pub mod variant_input;
