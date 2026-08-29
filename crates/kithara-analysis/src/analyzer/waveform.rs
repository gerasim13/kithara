use kithara_bufpool::SamplePool;

use crate::waveform::{AnalysisParams, WaveformAnalyzer, bucket::Waveform};

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

    pub(crate) fn push(&mut self, pcm: &[f32], channels: usize, at: u64) {
        self.inner.push(pcm, channels, at);
    }

    pub(crate) fn snapshot(&mut self, extent: Option<u64>) -> Waveform {
        self.inner.snapshot(self.buckets, extent)
    }
}
