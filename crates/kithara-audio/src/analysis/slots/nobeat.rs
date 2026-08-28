use std::{marker::PhantomData, num::NonZeroU32};

use kithara_bufpool::PcmPool;
use kithara_resampler::ResamplerBackend;

use crate::{analysis::BeatAnalysisConfig, coverage::FrameRange, waveform::BeatGrid};

pub(crate) type Detector = ();

pub(crate) struct Config<B>(PhantomData<B>);

impl<B> Config<B>
where
    B: ResamplerBackend,
{
    pub(crate) fn build(_config: &Self, _rate: NonZeroU32, _pcm_pool: &PcmPool) -> Slot<B> {
        Slot(PhantomData)
    }

    pub(crate) const fn is_empty(_config: &Self) -> bool {
        true
    }

    pub(crate) fn set_resampler(_config: &mut Self, _resampler: BeatAnalysisConfig<B>) {}

    pub(crate) fn take_detector(_config: &mut Self) -> Option<Detector> {
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
    pub(crate) fn snapshot(
        _slot: &mut Self,
        _detector: Option<&mut Detector>,
        _ending: bool,
        _extent: Option<u64>,
    ) -> Option<(BeatGrid, Vec<FrameRange>)> {
        None
    }

    pub(crate) fn push(
        _slot: &mut Self,
        _pcm: &[f32],
        _channels: usize,
        _at: u64,
        _detector: Option<&mut Detector>,
    ) {
    }
}
