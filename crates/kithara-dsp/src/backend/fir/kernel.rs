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
        for (frame, (&sample, peak)) in current.iter().zip(peaks).take(len).enumerate() {
            *peak = sample.abs().max(0.0);
            if self.reconstruct {
                let ahead = Self::interval(&self.taps, history, raw, sample, frame, frame as isize);
                let behind = if *valid {
                    *shared
                } else {
                    Self::interval(
                        &self.taps,
                        history,
                        raw,
                        sample,
                        frame,
                        (frame as isize).saturating_sub(1),
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
        history: &[f32; HISTORY],
        raw: &[f32],
        current: f32,
        frame: usize,
        base: isize,
    ) -> f32 {
        let mut peak = 0.0_f32;
        for branch in taps {
            let mut sum = 0.0_f32;
            for (tap, coefficient) in branch.iter().enumerate() {
                let position = base
                    .saturating_add(tap as isize)
                    .saturating_sub(HISTORY.saturating_sub(1) as isize);
                let sample = if position < frame as isize {
                    usize::try_from((frame as isize).saturating_sub(position))
                        .ok()
                        .and_then(|age| HISTORY.checked_sub(age))
                        .and_then(|slot| history.get(slot).copied())
                        .unwrap_or(0.0)
                } else if position == frame as isize {
                    current
                } else {
                    usize::try_from(position)
                        .ok()
                        .and_then(|index| {
                            raw.get(index).copied().or_else(|| {
                                if frame.checked_add(1) == Some(raw.len()) {
                                    Some(current)
                                } else {
                                    raw.last().copied()
                                }
                            })
                        })
                        .unwrap_or(0.0)
                };
                sum = coefficient.mul_add(sample, sum);
            }
            peak = peak.max(sum.abs());
        }
        peak
    }
}
