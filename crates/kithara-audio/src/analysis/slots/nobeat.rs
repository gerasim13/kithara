use std::marker::PhantomData;

use kithara_bufpool::SamplePool;
use kithara_resampler::ResamplerBackend;
use kithara_signal::{AudioChunk, AudioSpec};

use crate::{analysis::BeatAnalysisConfig, waveform::BeatGrid};

pub(crate) type Detector = ();

pub(crate) struct Config<B>(PhantomData<B>);

impl<B> Config<B>
where
    B: ResamplerBackend,
{
    pub(crate) fn build(_config: &Self, _spec: AudioSpec, _sample_pool: &SamplePool) -> Slot<B> {
        Slot(PhantomData)
    }

    pub(crate) const fn is_empty(_config: &Self) -> bool {
        true
    }

    pub(crate) fn set_resampler(_config: &mut Self, _resampler: BeatAnalysisConfig<B>) {}

    pub(crate) fn take_detector(_config: &mut Self, _sample_pool: &SamplePool) -> Option<Detector> {
        None
    }

    pub(crate) fn with_default(_config: &mut Self, _resampler: BeatAnalysisConfig<B>) {}
}

impl<B> Default for Config<B> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

pub(crate) struct Slot<B>(PhantomData<B>);

impl<B> Default for Slot<B> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<B> Slot<B>
where
    B: ResamplerBackend,
{
    pub(crate) fn finish(_slot: Self, _detector: Option<&mut Detector>) -> Option<BeatGrid> {
        None
    }

    pub(crate) const fn is_empty(_slot: &Self) -> bool {
        true
    }

    pub(crate) fn push(_slot: &mut Self, _chunk: &AudioChunk, _detector: Option<&mut Detector>) {}
}
