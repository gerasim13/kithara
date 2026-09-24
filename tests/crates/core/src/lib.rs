#![forbid(unsafe_code)]

//! Deterministic PCM inputs shared by low-level unit tests.

mod pcm;

pub use pcm::{channel_signals, negative_pcm_ramp, pcm_ramp, silence_pcm, stereo_pair, trim_ramp};
