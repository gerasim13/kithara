mod common;

pub mod analysis;
pub mod beat;
#[cfg(not(target_arch = "wasm32"))]
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
    provenance_silence, silence_pcm, stereo_pair, stress_wav, tone_mp3,
};
#[cfg(not(target_arch = "wasm32"))]
pub use common::{decoder_wav, seek_decoder_wav, short_decoder_wav, tone_wav};
