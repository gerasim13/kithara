#[cfg(not(target_arch = "wasm32"))]
mod bytes;
#[cfg(not(target_arch = "wasm32"))]
mod codec;
mod gapless;
#[cfg(not(target_arch = "wasm32"))]
mod mux;

pub use gapless::GaplessEncoding;
#[cfg(not(target_arch = "wasm32"))]
pub use mux::{Fmp4MuxError, Fmp4Package, mux_audio_track, mux_audio_track_at};
