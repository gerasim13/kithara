use std::num::NonZeroU32;

use kithara_bufpool::SamplePool;
use kithara_resampler::ResamplerBackend;

use crate::{
    BeatArtifact, BlobError,
    analyzer::{BeatAnalysisConfig, default_beat_detector},
    beat::{BeatDetector, BeatPass, BeatPassConfig, DetectOutput, DetectRequest, GridParams},
    coverage::FrameRange,
    progress::BeatResume,
};

pub(crate) type Detector = Box<dyn BeatDetector>;

struct BeatConfig<B>
where
    B: ResamplerBackend,
{
    resampler: BeatAnalysisConfig<B>,
    params: GridParams,
    detector: Option<DetectorConfig>,
}

enum DetectorConfig {
    Default,
    #[cfg(test)]
    Custom(Detector),
}

pub(crate) struct Config<B>(Option<BeatConfig<B>>)
where
    B: ResamplerBackend;

impl<B> Config<B>
where
    B: ResamplerBackend,
{
    pub(crate) fn build(&self, rate: NonZeroU32, sample_pool: &SamplePool) -> Slot<B> {
        Slot(self.0.as_ref().map(|config| {
            let pass = BeatPassConfig::builder()
                .source_rate(rate.get())
                .params(config.params.clone())
                .resampler(config.resampler.clone())
                .sample_pool(sample_pool.clone())
                .build();
            BeatPass::new(pass)
        }))
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    pub(crate) fn take_detector(&mut self, sample_pool: &SamplePool) -> Option<Detector> {
        let source = self.0.as_mut()?.detector.take()?;
        let detector = match source {
            DetectorConfig::Default => default_beat_detector(sample_pool),
            #[cfg(test)]
            DetectorConfig::Custom(detector) => Some(detector),
        };
        if detector.is_none() {
            self.0 = None;
        }
        detector
    }

    pub(crate) fn set_resampler(&mut self, resampler: BeatAnalysisConfig<B>) {
        if let Some(config) = &mut self.0 {
            config.resampler = resampler;
        }
    }

    pub(crate) fn with_default(&mut self, resampler: BeatAnalysisConfig<B>) {
        self.0 = Some(BeatConfig {
            resampler,
            detector: Some(DetectorConfig::Default),
            params: GridParams::default(),
        });
    }

    #[cfg(test)]
    pub(crate) fn with_detector(
        &mut self,
        detector: Detector,
        params: GridParams,
        resampler: BeatAnalysisConfig<B>,
    ) {
        self.0 = Some(BeatConfig {
            params,
            resampler,
            detector: Some(DetectorConfig::Custom(detector)),
        });
    }
}

impl<B> Default for Config<B>
where
    B: ResamplerBackend,
{
    fn default() -> Self {
        Self(None)
    }
}

pub(crate) struct Slot<B>(Option<BeatPass<B>>)
where
    B: ResamplerBackend;

impl<B> Default for Slot<B>
where
    B: ResamplerBackend,
{
    fn default() -> Self {
        Self(None)
    }
}

impl<B> Slot<B>
where
    B: ResamplerBackend,
{
    pub(crate) fn snapshot(
        &mut self,
        detector: Option<&mut Detector>,
        ending: bool,
        extent: Option<u64>,
    ) -> Option<(BeatArtifact, Vec<FrameRange>)> {
        let analyzer = self.0.as_mut()?;
        match detector {
            Some(detector) => analyzer.snapshot(detector.as_mut(), ending, extent),
            None => analyzer.snapshot_deferred(ending, extent),
        }
    }

    pub(crate) fn push(
        &mut self,
        pcm: &[f32],
        channels: usize,
        at: u64,
        detector: Option<&mut Detector>,
    ) {
        if let Some(analyzer) = &mut self.0 {
            match detector {
                Some(detector) => analyzer.push(pcm, channels, at, detector.as_mut()),
                None => analyzer.push_deferred(pcm, channels, at),
            }
        }
    }

    pub(crate) fn prepare_detection(&mut self, trailing: bool) -> Option<DetectRequest> {
        self.0.as_mut()?.prepare_detection(trailing)
    }

    pub(crate) fn apply_detection(&mut self, output: DetectOutput) {
        if let Some(analyzer) = &mut self.0 {
            analyzer.apply_detection(output);
        }
    }

    pub(crate) fn write_resume(&mut self) -> Option<Vec<u8>> {
        self.0.as_mut().map(|analyzer| {
            let mut out = Vec::new();
            analyzer.write_resume(&mut out);
            out
        })
    }

    pub(crate) fn restore(&mut self, resume: Option<BeatResume>) -> Result<(), BlobError> {
        match (self.0.as_mut(), resume) {
            (Some(analyzer), Some(resume)) => analyzer.restore(resume),
            (None, None) => Ok(()),
            (Some(_), None) | (None, Some(_)) => Err(BlobError::Corrupt),
        }
    }
}

pub(crate) use crate::beat::{DetectOutput as DetectionOutput, DetectRequest as DetectionRequest};

pub(crate) fn detect(request: DetectionRequest, detector: &mut Detector) -> DetectionOutput {
    request.detect(detector.as_mut())
}
