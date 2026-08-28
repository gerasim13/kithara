use std::num::NonZeroU32;

use kithara_bufpool::SamplePool;

use crate::{analysis::analyzer::WaveformPass, waveform::bucket::Waveform};

pub(crate) type Config = Option<usize>;
pub(crate) type Slot = Option<WaveformPass>;

pub(crate) fn build(config: &Config, rate: NonZeroU32, sample_pool: &SamplePool) -> Slot {
    config
        .as_ref()
        .map(|buckets| WaveformPass::new(rate.get(), *buckets, sample_pool))
}

pub(crate) fn cache_tag(config: &Config) -> Option<String> {
    config.map(|buckets| format!("wave:native:max{buckets}:v1"))
}

pub(crate) const fn config_is_empty(config: &Config) -> bool {
    config.is_none()
}

pub(crate) fn push(slot: &mut Slot, pcm: &[f32], channels: usize, at: u64) {
    if let Some(analyzer) = slot {
        analyzer.push(pcm, channels, at);
    }
}

pub(crate) fn snapshot(slot: &mut Slot, extent: Option<u64>) -> Option<Waveform> {
    slot.as_mut().map(|analyzer| analyzer.snapshot(extent))
}

pub(crate) const fn with_buckets(config: &mut Config, buckets: usize) {
    *config = Some(buckets);
}
