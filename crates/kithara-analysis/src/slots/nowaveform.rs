use std::num::NonZeroU32;

use kithara_bufpool::SamplePool;

use crate::{BlobError, progress::WaveformResume, waveform::bucket::Waveform};

#[derive(Default)]
pub(crate) struct Config;

#[derive(Default)]
pub(crate) struct Slot;

pub(crate) fn build(_config: &Config, _rate: NonZeroU32, _sample_pool: &SamplePool) -> Slot {
    Slot
}

pub(crate) const fn cache_tag(_config: &Config) -> Option<String> {
    None
}

pub(crate) const fn config_is_empty(_config: &Config) -> bool {
    true
}

pub(crate) fn push(_slot: &mut Slot, _pcm: &[f32], _channels: usize, _at: u64) {}

pub(crate) fn snapshot(_slot: &mut Slot, _extent: Option<u64>) -> Option<Waveform> {
    None
}

pub(crate) const fn write_resume(_slot: &Slot) -> Option<Vec<u8>> {
    None
}

pub(crate) fn restore(_slot: &mut Slot, resume: Option<WaveformResume>) -> Result<(), BlobError> {
    if resume.is_none() {
        Ok(())
    } else {
        Err(BlobError::Corrupt)
    }
}
