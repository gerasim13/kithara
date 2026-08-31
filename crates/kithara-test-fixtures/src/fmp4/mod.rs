mod bytes;
mod codec;
mod gapless;
mod mux;

pub use gapless::GaplessEncoding;
pub use mux::{Fmp4MuxError, Fmp4Package, mux_audio_track, mux_audio_track_at};
