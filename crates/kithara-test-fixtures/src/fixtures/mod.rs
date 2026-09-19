mod common;
mod signal;
mod wav;

pub mod analysis;
pub mod beat;
#[cfg(all(feature = "hls-inputs", not(target_arch = "wasm32")))]
pub mod hls;
pub mod integration;
pub mod mock;
pub mod play;
pub mod stretch;
pub mod unit;

pub(crate) use common::samples;
pub use common::{
    ascending_pcm, ascending_wrap_pcm, channel_signals, descending_pcm, descending_wrap_pcm,
    direction_channel_less, direction_step, negative_pcm_ramp, pcm_ramp, phase_endpoints,
    provenance_silence, silence_pcm, stereo_pair,
};
#[cfg(feature = "signal")]
pub use signal::tone_mp3;
#[cfg(all(feature = "signal", not(target_arch = "wasm32")))]
pub use signal::tone_wav;
#[cfg(feature = "wav")]
pub use wav::stress_wav;
#[cfg(all(feature = "wav", not(target_arch = "wasm32")))]
pub use wav::{decoder_wav, seek_decoder_wav, short_decoder_wav};
