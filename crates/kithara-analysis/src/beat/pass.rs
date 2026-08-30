use kithara_bufpool::{HasPool, PoolRegion};
use kithara_resampler::ResamplerBackend;
use tracing::warn;

use super::{
    analyzer::{BeatAnalyzer, BeatPassConfig},
    detector::BeatDetector,
    grid::extend_over,
};
use crate::{BeatArtifact, coverage::FrameRange};

pub(crate) struct BeatPass<B>
where
    B: ResamplerBackend,
{
    analyzer: BeatAnalyzer<B>,
}

impl<B> BeatPass<B>
where
    B: ResamplerBackend,
{
    pub(crate) fn new<S>(config: BeatPassConfig<B, S>) -> Self
    where
        S: HasPool<f32>,
    {
        Self {
            analyzer: BeatAnalyzer::new(config),
        }
    }

    pub(crate) fn push<S>(
        &mut self,
        pools: &PoolRegion<S>,
        pcm: &[f32],
        channels: usize,
        at: u64,
        detector: &mut dyn BeatDetector,
    ) where
        S: HasPool<f32>,
    {
        self.analyzer
            .push_interleaved(pools, pcm, channels, at, detector);
    }

    pub(crate) fn snapshot(
        &mut self,
        detector: &mut dyn BeatDetector,
        ending: bool,
        extent: Option<u64>,
    ) -> Option<(BeatArtifact, Vec<FrameRange>)> {
        match self.analyzer.snapshot(detector, ending) {
            Ok(grid) => {
                let rate = self.analyzer.source_rate();
                let grid = match extent {
                    Some(extent) => extend_over(grid, extent, rate),
                    None => grid,
                };
                Some((grid, self.analyzer.unanalysed()))
            }
            Err(e) => {
                warn!(?e, "beat analysis failed; leaving the beat slot empty");
                None
            }
        }
    }
}
