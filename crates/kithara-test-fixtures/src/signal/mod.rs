mod pcm;
mod riff;
mod wave;

pub use pcm::Pcm;
pub use riff::{header, wav, wav_from_fn, wav_of_size};
pub use wave::{SAW_PERIOD, SweepMode, TONE, Wave};
