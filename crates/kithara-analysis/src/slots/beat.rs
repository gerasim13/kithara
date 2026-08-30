use std::num::NonZeroU32;

use kithara_bufpool::{HasPool, PoolRegion};
use kithara_resampler::ResamplerBackend;

use crate::{
    BeatArtifact,
    analyzer::{BeatAnalysisConfig, default_beat_detector},
    beat::{BeatDetector, BeatPass, BeatPassConfig, GridParams},
    coverage::FrameRange,
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
    pub(crate) fn build<S>(&self, rate: NonZeroU32, pools: &PoolRegion<S>) -> Slot<B>
    where
        S: HasPool<f32>,
    {
        Slot(self.0.as_ref().map(|config| {
            let pass = BeatPassConfig::builder()
                .source_rate(rate.get())
                .params(config.params.clone())
                .resampler(config.resampler.clone())
                .pools(pools.clone())
                .build();
            BeatPass::new(pass)
        }))
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    pub(crate) fn take_detector<S>(&mut self, pools: &PoolRegion<S>) -> Option<Detector>
    where
        S: HasPool<f32> + Send + Sync + 'static,
    {
        let source = self.0.as_mut()?.detector.take()?;
        let detector = match source {
            DetectorConfig::Default => default_beat_detector(pools),
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
        let (analyzer, detector) = (self.0.as_mut()?, detector?);
        analyzer.snapshot(detector.as_mut(), ending, extent)
    }

    pub(crate) fn push<S>(
        &mut self,
        pools: &PoolRegion<S>,
        pcm: &[f32],
        channels: usize,
        at: u64,
        detector: Option<&mut Detector>,
    ) where
        S: HasPool<f32>,
    {
        if let (Some(analyzer), Some(detector)) = (&mut self.0, detector) {
            analyzer.push(pools, pcm, channels, at, detector.as_mut());
        }
    }
}
