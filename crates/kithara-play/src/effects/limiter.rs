use core::num::{NonZeroU32, NonZeroUsize};

use kithara_signal::sanitize_sample;
use num_traits::ToPrimitive;

/// Milliseconds per second: the release time arrives in ms, the coefficient
/// is computed in samples.
const MS_PER_SEC: f32 = 1000.0;

/// Configuration rejected by [`PeakLimiter::new`].
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum LimiterError {
    #[error("limiter ceiling {ceiling} is not finite in (0.0, 1.0]")]
    Ceiling { ceiling: f32 },

    #[error("limiter release {release_ms} ms is not finite and positive")]
    Release { release_ms: f32 },
}

/// Stereo-linked, zero-lookahead peak limiter: immediate attack, exponential release toward unity,
/// and a unity bypass below the ceiling that is exact for normal finite samples.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PeakLimiter {
    ceiling: f32,
    envelope: f32,
    release_coeff: f32,
    channels: usize,
}

impl PeakLimiter {
    /// Build a limiter with a linear `ceiling` and `release_ms` recovery.
    ///
    /// # Errors
    /// Returns [`LimiterError`] when `ceiling` is outside `(0.0, 1.0]` or
    /// `release_ms` is not finite and positive.
    pub fn new(
        sample_rate: NonZeroU32,
        channels: NonZeroUsize,
        ceiling: f32,
        release_ms: f32,
    ) -> Result<Self, LimiterError> {
        if !ceiling.is_finite() || ceiling <= 0.0 || ceiling > 1.0 {
            return Err(LimiterError::Ceiling { ceiling });
        }
        if !release_ms.is_finite() || release_ms <= 0.0 {
            return Err(LimiterError::Release { release_ms });
        }

        let samples = release_ms / MS_PER_SEC * sample_rate.get().to_f32().unwrap_or(1.0);
        let release_coeff = (-1.0 / samples).exp();

        Ok(Self {
            ceiling,
            release_coeff,
            envelope: 1.0,
            channels: channels.get(),
        })
    }

    /// Apply the limiter in place to a planar block, linking channels by frame peak. Each sample is
    /// guarded before the peak is taken, so the envelope only ever sees a finite peak. Allocates
    /// nothing, locks nothing, performs no I/O.
    pub fn process_planar(&mut self, channels: &mut [&mut [f32]]) {
        debug_assert_eq!(channels.len(), self.channels);

        let frames = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        for frame in 0..frames {
            let mut peak = 0.0_f32;
            for channel in channels.iter_mut() {
                let sample = sanitize_sample(channel[frame]);
                channel[frame] = sample;
                peak = peak.max(sample.abs());
            }
            let gain = self.step(peak);
            for channel in channels.iter_mut() {
                channel[frame] *= gain;
            }
        }
    }

    /// Reset the gain envelope to unity.
    pub const fn reset(&mut self) {
        self.envelope = 1.0;
    }

    #[inline]
    fn step(&mut self, peak: f32) -> f32 {
        let required = if peak > self.ceiling {
            self.ceiling / peak
        } else {
            1.0
        };
        // WHY: Release before the clamp: the reverse order lets the recovered gain overshoot the ceiling for one frame.
        self.envelope = (1.0 - self.envelope).mul_add(-self.release_coeff, 1.0);
        if required < self.envelope {
            self.envelope = required;
        }
        self.envelope
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_fixtures::unit_fixtures::{
        limiter_attack, limiter_half, limiter_infinity, limiter_left, limiter_negative,
        limiter_negative_infinity, limiter_peak, limiter_quiet, limiter_recovery, limiter_right,
        limiter_silence, limiter_spike, limiter_two, limiter_unity,
    };
    use kithara_test_utils::kithara;

    use super::*;

    const CEILING: f32 = 0.98;

    fn limiter(sample_rate: u32, release_ms: f32) -> PeakLimiter {
        PeakLimiter::new(
            NonZeroU32::new(sample_rate).unwrap(),
            NonZeroUsize::new(2).unwrap(),
            CEILING,
            release_ms,
        )
        .unwrap()
    }

    fn run(limiter: &mut PeakLimiter, left: &mut [f32], right: &mut [f32]) {
        let mut chans: [&mut [f32]; 2] = [left, right];
        limiter.process_planar(&mut chans);
    }

    #[kithara::test(native, flash(false))]
    fn below_ceiling_is_bit_exact_unity(limiter_unity: Vec<f32>) {
        let mut lim = limiter(44_100, 50.0);
        let input = limiter_unity.clone();
        let mut left = input.clone();
        let mut right = input.clone();
        run(&mut lim, &mut left, &mut right);
        assert_eq!(left, input);
        assert_eq!(right, input);
    }

    #[kithara::test(native, flash(false))]
    fn positive_peak_clamped_to_ceiling(limiter_peak: Vec<f32>) {
        let mut lim = limiter(44_100, 50.0);
        let mut left = limiter_peak.clone();
        let mut right = limiter_peak.clone();
        run(&mut lim, &mut left, &mut right);
        for &s in &left {
            assert!(
                (s - CEILING).abs() < 1e-6,
                "sample {s} not clamped to ceiling"
            );
        }
    }

    #[kithara::test(native, flash(false))]
    fn negative_peak_clamped_to_ceiling(limiter_negative: Vec<f32>) {
        let mut lim = limiter(44_100, 50.0);
        let mut left = limiter_negative.clone();
        let mut right = limiter_negative.clone();
        run(&mut lim, &mut left, &mut right);
        for &s in &left {
            assert!(
                (s + CEILING).abs() < 1e-6,
                "sample {s} not clamped to -ceiling"
            );
        }
    }

    #[kithara::test(native, flash(false))]
    fn no_sample_exceeds_ceiling_over_varied_input(
        limiter_right: Vec<f32>,
        limiter_left: Vec<f32>,
    ) {
        let mut lim = limiter(48_000, 50.0);
        let mut left = limiter_left.clone();
        let mut right = limiter_right.clone();
        {
            let mut chans: [&mut [f32]; 2] = [&mut left, &mut right];
            lim.process_planar(&mut chans);
        }
        for (&l, &r) in left.iter().zip(right.iter()) {
            assert!(l.abs() <= CEILING + 1e-6, "left {l} over ceiling");
            assert!(r.abs() <= CEILING + 1e-6, "right {r} over ceiling");
        }
    }

    #[kithara::test(native, flash(false))]
    fn channels_link_by_frame_peak(limiter_quiet: Vec<f32>, limiter_two: Vec<f32>) {
        let mut lim = limiter(44_100, 50.0);
        let mut left = limiter_two.clone();
        let mut right = limiter_quiet.clone();
        run(&mut lim, &mut left, &mut right);
        let gain = CEILING / 2.0;
        assert!(2.0f32.mul_add(-gain, left[0]).abs() < 1e-6);
        assert!(0.1f32.mul_add(-gain, right[0]).abs() < 1e-6);
    }

    #[kithara::test(native, flash(false))]
    fn attack_is_immediate_from_first_frame(limiter_attack: Vec<f32>) {
        let mut lim = limiter(44_100, 50.0);
        let mut left = limiter_attack.clone();
        let mut right = limiter_attack.clone();
        run(&mut lim, &mut left, &mut right);
        assert!((left[0] - CEILING).abs() < 1e-6, "first frame not limited");
    }

    #[kithara::test(native, flash(false))]
    fn release_recovers_monotonically_toward_unity(
        limiter_half: Vec<f32>,
        limiter_spike: Vec<f32>,
    ) {
        let mut lim = limiter(44_100, 50.0);
        let mut spike_l = limiter_spike.clone();
        let mut spike_r = limiter_spike.clone();
        run(&mut lim, &mut spike_l, &mut spike_r);

        let signal = 0.5_f32;
        let mut prev_gain = 0.0_f32;
        for _ in 0..20_000 {
            let mut l = limiter_half.clone();
            let mut r = limiter_half.clone();
            run(&mut lim, &mut l, &mut r);
            let gain = l[0] / signal;
            assert!(
                gain >= prev_gain - 1e-7,
                "gain went backwards: {prev_gain} -> {gain}"
            );
            assert!(gain <= 1.0 + 1e-7);
            prev_gain = gain;
        }
        assert!(
            prev_gain > 0.99,
            "did not recover toward unity: {prev_gain}"
        );
    }

    #[kithara::test(native, flash(false))]
    fn release_slope_is_sample_rate_derived(limiter_half: Vec<f32>, limiter_spike: Vec<f32>) {
        let drive = |lim: &mut PeakLimiter| {
            let mut sl = limiter_spike.clone();
            let mut sr = limiter_spike.clone();
            run(lim, &mut sl, &mut sr);
        };
        let recover_after = |lim: &mut PeakLimiter, frames: usize| -> f32 {
            let signal = 0.5_f32;
            let mut gain = 0.0;
            for _ in 0..frames {
                let mut l = limiter_half.clone();
                let mut r = limiter_half.clone();
                run(lim, &mut l, &mut r);
                gain = l[0] / signal;
            }
            gain
        };

        let mut slow = limiter(44_100, 50.0);
        let mut fast = limiter(96_000, 50.0);
        drive(&mut slow);
        drive(&mut fast);
        let g_low = recover_after(&mut slow, 500);
        let g_high = recover_after(&mut fast, 500);
        assert!(
            g_low > g_high,
            "expected slower per-frame recovery at 96k: {g_low} !> {g_high}"
        );
    }

    #[kithara::test(native, flash(false))]
    fn silence_stays_silent_and_finite(limiter_silence: Vec<f32>) {
        let mut lim = limiter(44_100, 50.0);
        let mut left = limiter_silence.clone();
        let mut right = limiter_silence.clone();
        run(&mut lim, &mut left, &mut right);
        for &s in left.iter().chain(right.iter()) {
            assert_eq!(s, 0.0);
            assert!(s.is_finite());
        }
    }

    #[kithara::test(native, flash(false))]
    fn non_finite_frame_stays_silent_without_ducking_the_next_block(
        limiter_recovery: Vec<f32>,
        limiter_negative_infinity: Vec<f32>,
        limiter_infinity: Vec<f32>,
    ) {
        let mut lim = limiter(48_000, 50.0);
        let mut spike_l = limiter_infinity.clone();
        let mut spike_r = limiter_negative_infinity.clone();
        run(&mut lim, &mut spike_l, &mut spike_r);
        assert_eq!(spike_l[0], 0.0);
        assert_eq!(spike_r[0], 0.0);

        let signal = 0.5_f32;
        let mut left = limiter_recovery.clone();
        let mut right = limiter_recovery.clone();
        run(&mut lim, &mut left, &mut right);
        assert_eq!(left, [signal; 64], "the block after lost level");
        assert_eq!(right, [signal; 64], "the block after lost level");
    }

    #[kithara::test(native, flash(false))]
    fn invalid_config_is_rejected() {
        let sr = NonZeroU32::new(44_100).unwrap();
        let ch = NonZeroUsize::new(2).unwrap();
        assert!(PeakLimiter::new(sr, ch, 0.0, 50.0).is_err());
        assert!(PeakLimiter::new(sr, ch, 1.5, 50.0).is_err());
        assert!(PeakLimiter::new(sr, ch, f32::NAN, 50.0).is_err());
        assert!(PeakLimiter::new(sr, ch, CEILING, 0.0).is_err());
        assert!(PeakLimiter::new(sr, ch, CEILING, -5.0).is_err());
        assert!(PeakLimiter::new(sr, ch, CEILING, f32::INFINITY).is_err());
    }
}
