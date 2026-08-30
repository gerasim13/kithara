use kithara_bufpool::SamplePool;

use crate::{
    BlobError,
    progress::WaveformResume,
    waveform::{AnalysisParams, WaveformAnalyzer, bucket::Waveform},
};

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

    delegate::delegate! {
        to self.inner {
            pub(crate) fn push(&mut self, pcm: &[f32], channels: usize, at: u64);
            pub(crate) fn write_resume(&self, out: &mut Vec<u8>);
            pub(crate) fn restore(&mut self, resume: WaveformResume) -> Result<(), BlobError>;
        }
    }

    pub(crate) fn snapshot(&mut self, extent: Option<u64>) -> Waveform {
        self.inner.snapshot(self.buckets, extent)
    }
}
