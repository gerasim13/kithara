use std::num::NonZeroU32;

use kithara_bufpool::PcmPool;
use kithara_resampler::ResamplerBackend;

use crate::{
    analysis::{
        analyzer::{BeatAnalysisConfig, default_beat_detector},
        beat::{BeatDetector, BeatPass, BeatPassConfig, GridParams},
    },
    coverage::FrameRange,
    waveform::BeatGrid,
};

pub(crate) type Detector = Box<dyn BeatDetector>;

struct BeatConfig<B>
where
    B: ResamplerBackend,
{
    resampler: BeatAnalysisConfig<B>,
    params: GridParams,
    detector: Option<Detector>,
}

pub(crate) struct Config<B>(Option<BeatConfig<B>>)
where
    B: ResamplerBackend;

impl<B> Config<B>
where
    B: ResamplerBackend,
{
    pub(crate) fn build(&self, rate: NonZeroU32, pcm_pool: &PcmPool) -> Slot<B> {
        Slot(self.0.as_ref().map(|config| {
            let pass = BeatPassConfig::builder()
                .source_rate(rate.get())
                .params(config.params.clone())
                .resampler(config.resampler.clone())
                .pcm_pool(pcm_pool.clone())
                .build();
            BeatPass::new(pass)
        }))
    }

    delegate::delegate! {
        to self.0 {
            #[call(is_none)]
            pub(crate) const fn is_empty(&self) -> bool;
            #[expr($.and_then(|config| config.detector.take()))]
            #[call(as_mut)]
            pub(crate) fn take_detector(&mut self) -> Option<Detector>;
        }
    }

    pub(crate) fn set_resampler(&mut self, resampler: BeatAnalysisConfig<B>) {
        if let Some(config) = &mut self.0 {
            config.resampler = resampler;
        }
    }

    pub(crate) fn with_default(&mut self, resampler: BeatAnalysisConfig<B>) {
        self.0 = default_beat_detector().map(|detector| BeatConfig {
            resampler,
            detector: Some(detector),
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
            detector: Some(detector),
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
    ) -> Option<(BeatGrid, Vec<FrameRange>)> {
        let (analyzer, detector) = (self.0.as_mut()?, detector?);
        analyzer.snapshot(detector.as_mut(), ending, extent)
    }

    pub(crate) fn push(
        &mut self,
        pcm: &[f32],
        channels: usize,
        at: u64,
        detector: Option<&mut Detector>,
    ) {
        if let (Some(analyzer), Some(detector)) = (&mut self.0, detector) {
            analyzer.push(pcm, channels, at, detector.as_mut());
        }
    }
}
