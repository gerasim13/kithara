use kithara_resampler::ResamplerBackend;
use tracing::warn;

use super::{
    analyzer::{BeatAnalyzer, BeatPassConfig, DetectOutput, DetectRequest},
    detector::BeatDetector,
    grid::extend_over,
};
use crate::{BeatArtifact, BlobError, coverage::FrameRange, progress::BeatResume};

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
    pub(crate) fn new(config: BeatPassConfig<B>) -> Self {
        Self {
            analyzer: BeatAnalyzer::new(config),
        }
    }

    delegate::delegate! {
        to self.analyzer {
            #[call(push_interleaved)]
            pub(crate) fn push(
                &mut self,
                pcm: &[f32],
                channels: usize,
                at: u64,
                detector: &mut dyn BeatDetector,
            );
            #[call(push_interleaved_deferred)]
            pub(crate) fn push_deferred(&mut self, pcm: &[f32], channels: usize, at: u64);
            pub(crate) fn prepare_detection(&mut self, trailing: bool) -> Option<DetectRequest>;
            pub(crate) fn apply_detection(&mut self, output: DetectOutput);
            pub(crate) fn write_resume(&mut self, out: &mut Vec<u8>);
            pub(crate) fn restore(&mut self, resume: BeatResume) -> Result<(), BlobError>;
        }
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

    pub(crate) fn snapshot_deferred(
        &mut self,
        ending: bool,
        extent: Option<u64>,
    ) -> Option<(BeatArtifact, Vec<FrameRange>)> {
        match self.analyzer.snapshot_deferred(ending) {
            Ok(grid) => {
                let rate = self.analyzer.source_rate();
                let grid = match extent {
                    Some(extent) => extend_over(grid, extent, rate),
                    None => grid,
                };
                Some((grid, self.analyzer.unanalysed()))
            }
            Err(error) => {
                warn!(?error, "beat analysis failed; leaving the beat slot empty");
                None
            }
        }
    }
}
