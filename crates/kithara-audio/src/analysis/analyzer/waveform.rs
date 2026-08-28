use kithara_bufpool::SamplePool;
use kithara_signal::AudioChunk;

use super::track::Analyzer;
use crate::waveform::{AnalysisParams, WaveformAnalyzer};

pub(crate) struct WaveformPass {
    inner: WaveformAnalyzer,
    buckets: usize,
}

impl WaveformPass {
    pub(crate) fn new(sample_rate: u32, buckets: usize, sample_pool: &SamplePool) -> Self {
        Self {
            buckets,
            inner: WaveformAnalyzer::new(sample_rate, AnalysisParams::default(), sample_pool),
        }
    }
}

impl Analyzer for WaveformPass {
    type Output = crate::waveform::bucket::Waveform;

    fn finish(self) -> Self::Output {
        self.inner.finalize(self.buckets)
    }

    fn push(&mut self, chunk: &AudioChunk) {
        let channels = usize::from(chunk.spec().channels.max(1));
        self.inner.push_interleaved(&chunk.samples[..], channels);
    }
}
