use fearless_simd::Simd;

use super::Fir;
use crate::envelope::PeakHold;

impl<const TAPS: usize, const PHASES: usize, const CHANNELS: usize, const HISTORY: usize>
    Fir<TAPS, PHASES, CHANNELS, HISTORY>
{
    #[inline(always)]
    pub(super) fn begin_block_kernel<S: Simd>(
        &mut self,
        _simd: S,
        channels: &[&mut [f32]],
        threshold: f32,
    ) {
        self.shared_valid.fill(false);
        if channels
            .iter()
            .map(|channel| channel.len())
            .min()
            .unwrap_or(0)
            == 0
        {
            self.reconstruct = false;
            return;
        }
        let maximum = channels
            .iter()
            .flat_map(|channel| channel.iter())
            .chain(self.history.iter().take(channels.len()).flatten())
            .fold(0.0_f32, |maximum, &sample| {
                maximum.max(if sample.is_finite() {
                    sample.abs()
                } else {
                    f32::INFINITY
                })
            });
        self.reconstruct = maximum > threshold
            || self.taps.iter().any(|branch| {
                branch.iter().fold(0.0_f32, |sum, coefficient| {
                    coefficient.abs().mul_add(maximum, sum)
                }) > threshold
            });
    }

    #[inline(always)]
    pub(super) fn process_kernel<S: Simd>(
        &mut self,
        _simd: S,
        channel: usize,
        raw: &[f32],
        current: &[f32],
        peaks: &mut [f32],
    ) -> usize {
        let (Some(history), Some(shared), Some(valid)) = (
            self.history.get_mut(channel),
            self.shared_peak.get_mut(channel),
            self.shared_valid.get_mut(channel),
        ) else {
            return 0;
        };
        let len = raw.len().min(current.len()).min(peaks.len());
        let mut future = raw.iter();
        for (&sample, peak) in current.iter().zip(peaks).take(len) {
            let _ = future.next();
            *peak = sample.abs().max(0.0);
            if self.reconstruct {
                let future = future.as_slice();
                let ahead =
                    Self::interval(&self.taps, &history[HISTORY.min(1)..], sample, future, 0);
                let behind = if *valid {
                    *shared
                } else {
                    Self::interval(
                        &self.taps,
                        history,
                        sample,
                        future,
                        usize::from(HISTORY == 0),
                    )
                };
                *shared = ahead;
                *peak = peak.max(PeakHold::peak(&[ahead, behind]));
            }
            if !history.is_empty() {
                history.copy_within(1.., 0);
                if let Some(newest) = history.last_mut() {
                    *newest = sample;
                }
            }
            *valid = true;
        }
        len
    }

    #[inline(always)]
    fn interval(
        taps: &[[f32; TAPS]; PHASES],
        past: &[f32],
        current: f32,
        future: &[f32],
        padding: usize,
    ) -> f32 {
        let mut peak = 0.0_f32;
        let held = future.last().copied().unwrap_or(current);
        for branch in taps {
            let mut sum = 0.0_f32;
            let mut coefficients = branch.iter();
            for coefficient in coefficients.by_ref().take(padding) {
                sum = coefficient.mul_add(0.0, sum);
            }
            // Bound zip before it can consume the next region's coefficient.
            for (coefficient, &sample) in coefficients.by_ref().take(past.len()).zip(past) {
                sum = coefficient.mul_add(sample, sum);
            }
            if let Some(coefficient) = coefficients.next() {
                sum = coefficient.mul_add(current, sum);
            }
            for (coefficient, &sample) in coefficients.by_ref().take(future.len()).zip(future) {
                sum = coefficient.mul_add(sample, sum);
            }
            for coefficient in coefficients {
                sum = coefficient.mul_add(held, sum);
            }
            peak = peak.max(sum.abs());
        }
        peak
    }
}
