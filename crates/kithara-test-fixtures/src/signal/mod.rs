mod detect;
mod pcm;
pub mod phase;
mod provenance;
mod riff;
mod tone;
mod wave;

pub use detect::{SignalDirection, detect_direction};
pub use pcm::Pcm;
pub use provenance::{FrameClass, Replay, ascending_phase_replays, classify_windows};
pub use riff::{header, wav, wav_from_fn, wav_of_size};
pub use tone::goertzel_magnitude;
pub use wave::{SAW_PERIOD, SweepMode, TONE, Wave};
