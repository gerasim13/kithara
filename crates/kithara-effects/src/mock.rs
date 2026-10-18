/// Reconstructs the continuous waveform by windowed-sinc interpolation and
/// returns its largest magnitude: the true peak a limiter ceiling bounds,
/// measured independently of the limiter's own detector.
/// The trailing window is omitted because its decay to silence is a finite-buffer
/// artifact, absent in the continuing stream. The real leading edge stays measured.
#[must_use]
pub fn reconstructed_peak(samples: &[f32]) -> f32 {
    const HALF_WIDTH: i16 = 32;
    let (sinc, window) = kithara_fixture_media::reconstruction::true_peak_kernel();
    let mut peak = 0.0_f32;
    let tail = usize::from(HALF_WIDTH.unsigned_abs());
    for index in 0..samples.len().saturating_sub(tail) {
        for (sinc, window) in sinc.iter().zip(window) {
            let mut value = 0.0_f32;
            for (tap, (&sinc, &window)) in (-HALF_WIDTH..=HALF_WIDTH).zip(sinc.iter().zip(window)) {
                let Some(&sample) = index
                    .checked_add_signed(isize::from(tap))
                    .and_then(|position| samples.get(position))
                else {
                    continue;
                };
                value += (sample * sinc) * window;
            }
            peak = peak.max(value.abs());
        }
    }
    peak
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::reconstructed_peak;

    const MIXED: [f32; 97] = {
        let pattern = [0.13, -0.71, 0.333_333_34, -0.125, 0.91, -0.001, 0.49];
        let mut samples = [0.0; 97];
        let mut index = 0;
        while index < samples.len() {
            samples[index] = pattern[index % pattern.len()];
            index += 1;
        }
        samples
    };
    const LEADING: [f32; 97] = {
        let mut samples = [0.0; 97];
        samples[0] = 0.73;
        samples[1] = -0.41;
        samples
    };
    const TRAILING: [f32; 97] = {
        let mut samples = [0.0; 97];
        samples[64] = -0.51;
        samples[95] = 0.37;
        samples[96] = -0.83;
        samples
    };

    fn scalar_reference(samples: &[f32]) -> f32 {
        let (sinc, window) = kithara_fixture_media::reconstruction::true_peak_kernel();
        let mut peak = 0.0_f32;
        for index in 0..samples.len().saturating_sub(32) {
            for phase in 0..16 {
                let mut value = 0.0_f32;
                for tap in -32_i16..=32 {
                    let Some(&sample) = index
                        .checked_add_signed(isize::from(tap))
                        .and_then(|position| samples.get(position))
                    else {
                        continue;
                    };
                    let column = usize::from((tap + 32).unsigned_abs());
                    value += sample * sinc[phase][column] * window[phase][column];
                }
                peak = peak.max(value.abs());
            }
        }
        peak
    }

    #[kithara::test]
    fn prepared_kernel_preserves_scalar_rounding_and_both_boundaries() {
        for samples in [&MIXED, &LEADING, &TRAILING] {
            for length in [0, 31, 32, 33, 64, 65, 97] {
                let samples = &samples[..length];
                assert_eq!(
                    reconstructed_peak(samples).to_bits(),
                    scalar_reference(samples).to_bits(),
                    "complete reconstructed peak for {length} samples"
                );
            }
        }
    }
}
