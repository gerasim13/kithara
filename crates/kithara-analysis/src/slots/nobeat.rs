use std::{marker::PhantomData, num::NonZeroU32};

use kithara_bufpool::{HasPool, PoolRegion};
use kithara_resampler::ResamplerBackend;

use crate::{
    BeatAnalysisConfig, BeatArtifact, BlobError, coverage::FrameRange, progress::BeatResume,
};

pub(crate) type Detector = ();

pub(crate) struct DetectionRequest;
pub(crate) struct DetectionOutput;

impl DetectionRequest {
    pub(crate) fn detect(self, _detector: &Detector) -> DetectionOutput {
        DetectionOutput
    }
}

pub(crate) fn detect(request: DetectionRequest, detector: &Detector) -> DetectionOutput {
    request.detect(detector)
}

#[derive(Clone, Copy)]
pub(crate) struct Config<B>(PhantomData<B>);

impl<B> Config<B>
where
    B: ResamplerBackend,
{
    pub(crate) fn build<S>(_config: &Self, _rate: NonZeroU32, _pools: &PoolRegion<S>) -> Slot<B>
    where
        S: HasPool<f32>,
    {
        Slot(PhantomData)
    }

    pub(crate) const fn is_empty(_config: &Self) -> bool {
        true
    }

    pub(crate) fn set_resampler(_config: &mut Self, _resampler: BeatAnalysisConfig<B>) {}

    pub(crate) fn take_detector<S>(_config: &mut Self, _pools: &PoolRegion<S>) -> Option<Detector>
    where
        S: HasPool<f32> + Send + Sync + 'static,
    {
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
    pub(crate) fn snapshot<S>(
        _slot: &mut Self,
        _pools: &PoolRegion<S>,
        _detector: Option<&mut Detector>,
        _ending: bool,
        _extent: Option<u64>,
    ) -> Option<(BeatArtifact, Vec<FrameRange>)>
    where
        S: HasPool<f32>,
    {
        None
    }

    pub(crate) fn push<S>(
        _slot: &mut Self,
        _pools: &PoolRegion<S>,
        _pcm: &[f32],
        _channels: usize,
        _at: u64,
        _detector: Option<&mut Detector>,
    ) where
        S: HasPool<f32>,
    {
    }

    pub(crate) fn prepare_detection<S>(
        &mut self,
        _pools: &PoolRegion<S>,
        _trailing: bool,
    ) -> Option<DetectionRequest>
    where
        S: HasPool<f32>,
    {
        None
    }

    pub(crate) fn apply_detection(&mut self, _output: DetectionOutput) {}

    pub(crate) const fn write_resume(&mut self) -> Option<Vec<u8>> {
        None
    }

    pub(crate) fn restore<S>(
        &mut self,
        _pools: &PoolRegion<S>,
        resume: Option<BeatResume>,
    ) -> Result<(), BlobError>
    where
        S: HasPool<f32>,
    {
        if resume.is_none() {
            Ok(())
        } else {
            Err(BlobError::Corrupt)
        }
    }
}
