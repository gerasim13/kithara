use std::{marker::PhantomData, num::NonZeroU32};

use kithara_bufpool::SamplePool;
use kithara_resampler::ResamplerBackend;

use crate::{
    BeatAnalysisConfig, BeatArtifact, BlobError, coverage::FrameRange, progress::BeatResume,
};

pub(crate) type Detector = ();

pub(crate) struct DetectionRequest;
pub(crate) struct DetectionOutput;

impl DetectionRequest {
    pub(crate) fn detect(self, _detector: &mut Detector) -> DetectionOutput {
        DetectionOutput
    }
}

pub(crate) fn detect(request: DetectionRequest, detector: &mut Detector) -> DetectionOutput {
    request.detect(detector)
}

pub(crate) struct Config<B>(PhantomData<B>);

impl<B> Config<B>
where
    B: ResamplerBackend,
{
    pub(crate) fn build(_config: &Self, _rate: NonZeroU32, _sample_pool: &SamplePool) -> Slot<B> {
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
    pub(crate) fn snapshot(
        _slot: &mut Self,
        _detector: Option<&mut Detector>,
        _ending: bool,
        _extent: Option<u64>,
    ) -> Option<(BeatArtifact, Vec<FrameRange>)> {
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

    pub(crate) fn prepare_detection(&mut self, _trailing: bool) -> Option<DetectionRequest> {
        None
    }

    pub(crate) fn apply_detection(&mut self, _output: DetectionOutput) {}

    pub(crate) const fn write_resume(&mut self) -> Option<Vec<u8>> {
        None
    }

    pub(crate) fn restore(&mut self, resume: Option<BeatResume>) -> Result<(), BlobError> {
        if resume.is_none() {
            Ok(())
        } else {
            Err(BlobError::Corrupt)
        }
    }
}
