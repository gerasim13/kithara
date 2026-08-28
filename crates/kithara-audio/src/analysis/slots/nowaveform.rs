use kithara_bufpool::SamplePool;
use kithara_signal::{AudioChunk, AudioSpec};

use crate::waveform::bucket::Waveform;

#[derive(Default)]
pub(crate) struct Config;

#[derive(Default)]
pub(crate) struct Slot;

pub(crate) fn build(_config: &Config, _spec: AudioSpec, _sample_pool: &SamplePool) -> Slot {
    Slot
}

pub(crate) const fn config_is_empty(_config: &Config) -> bool {
    true
}

pub(crate) fn finish(_slot: Slot) -> Option<Waveform> {
    None
}

pub(crate) fn push(_slot: &mut Slot, _chunk: &AudioChunk) {}
