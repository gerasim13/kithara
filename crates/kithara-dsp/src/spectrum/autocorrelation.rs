use std::{iter, num::NonZeroUsize};

use kithara_bufpool::{HasPool, PoolError, PoolRegion, SampleBuffer};
use num_traits::ToPrimitive;

use crate::backend::platform;

/// The autocorrelation of a frame over lags `0..N`, each lag divided by
/// its number of products `N − lag`: the unbiased estimate.
///
/// The frame is zero-extended to `N`, or cut to it, inside padding taken
/// once from the caller's region, so `process` does not allocate.
pub struct Autocorrelation {
    len: usize,
    padded: SampleBuffer,
}

impl Autocorrelation {
    /// # Errors
    /// [`PoolError`] when the padding does not fit the region.
    pub fn new<S>(len: NonZeroUsize, pools: &PoolRegion<S>) -> Result<Self, PoolError>
    where
        S: HasPool<f32>,
    {
        let len = len.get();
        Ok(Self {
            len,
            padded: pools.get_with_len::<f32>(len.saturating_mul(2).saturating_sub(1))?,
        })
    }

    /// Writes lags `0..N` of the first `N` samples of `frame` into
    /// `output`, as many as it holds; returns the lag count.
    pub fn process(&mut self, frame: &[f32], output: &mut [f32]) -> usize {
        let len = self.len;
        for (slot, sample) in self
            .padded
            .iter_mut()
            .take(len)
            .zip(frame.iter().copied().chain(iter::repeat(0.0)))
        {
            *slot = sample;
        }
        let Some(kernel) = self.padded.get(..len) else {
            return 0;
        };
        let lags = platform::correlate(&self.padded, kernel, output);
        for (lag, value) in output.iter_mut().take(lags).enumerate() {
            *value /= len.saturating_sub(lag).to_f32().unwrap_or(f32::NAN);
        }
        lags
    }
}
