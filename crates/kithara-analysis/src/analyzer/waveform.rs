use kithara_bufpool::{HasPool, PoolError, PoolRegion};

use crate::waveform::{AnalysisParams, WaveformAnalyzer, bucket::Waveform};

pub(crate) struct WaveformPass {
    inner: WaveformAnalyzer,
    buckets: usize,
}

impl WaveformPass {
    pub(crate) fn new<S>(
        sample_rate: u32,
        buckets: usize,
        pools: &PoolRegion<S>,
    ) -> Result<Self, PoolError>
    where
        S: HasPool<f32>,
    {
        Ok(Self {
            buckets,
            inner: WaveformAnalyzer::new(sample_rate, AnalysisParams::default(), pools)?,
        })
    }

    pub(crate) fn push<S>(
        &mut self,
        pools: &PoolRegion<S>,
        pcm: &[f32],
        channels: usize,
        at: u64,
    ) -> Result<(), PoolError>
    where
        S: HasPool<f32>,
    {
        self.inner.push(pools, pcm, channels, at)
    }

    pub(crate) fn snapshot(&mut self, extent: Option<u64>) -> Waveform {
        self.inner.snapshot(self.buckets, extent)
    }
}
