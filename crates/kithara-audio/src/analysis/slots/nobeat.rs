use std::marker::PhantomData;

use kithara_bufpool::PcmPool;
use kithara_decode::{PcmChunk, PcmSpec};
use kithara_resampler::ResamplerBackend;

use crate::{analysis::BeatAnalysisConfig, waveform::BeatGrid};

pub(crate) type Detector = ();

pub(crate) struct Config<B>(PhantomData<B>);

impl<B> Config<B>
where
    B: ResamplerBackend,
{
    pub(crate) fn build(_config: &Self, _spec: PcmSpec, _pcm_pool: &PcmPool) -> Slot<B> {
        Slot(PhantomData)
    }

    pub(crate) fn is_empty(_config: &Self) -> bool {
        true
    }

    pub(crate) fn set_resampler(_config: &mut Self, _resampler: BeatAnalysisConfig<B>) {}

    pub(crate) fn with_default(_config: &mut Self, _resampler: BeatAnalysisConfig<B>) {}

    pub(crate) fn take_detector(_config: &mut Self) -> Option<Detector> {
        None
    }
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

    pub(crate) fn is_empty(_slot: &Self) -> bool {
        true
    }

    pub(crate) fn push(_slot: &mut Self, _chunk: &PcmChunk, _detector: Option<&mut Detector>) {}
}
