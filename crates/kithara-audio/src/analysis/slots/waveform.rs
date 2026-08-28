use kithara_bufpool::SamplePool;
use kithara_signal::{AudioChunk, AudioSpec};

use crate::{
    analysis::analyzer::{Analyzer, WaveformPass},
    waveform::bucket::Waveform,
};

pub(crate) type Config = Option<usize>;
pub(crate) type Slot = Option<WaveformPass>;

pub(crate) fn build(config: &Config, spec: AudioSpec, sample_pool: &SamplePool) -> Slot {
    config
        .as_ref()
        .map(|buckets| WaveformPass::new(spec.sample_rate.get(), *buckets, sample_pool))
}

pub(crate) const fn config_is_empty(config: &Config) -> bool {
    config.is_none()
}

pub(crate) fn finish(slot: Slot) -> Option<Waveform> {
    slot.map(Analyzer::finish)
}

pub(crate) fn push(slot: &mut Slot, chunk: &AudioChunk) {
    if let Some(analyzer) = slot {
        analyzer.push(chunk);
    }
}

pub(crate) const fn with_buckets(config: &mut Config, buckets: usize) {
    *config = Some(buckets);
}
