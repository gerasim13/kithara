use core::f32::consts::PI;

/// Reconstructs the continuous waveform by windowed-sinc interpolation and
/// returns its largest magnitude: the true peak a limiter ceiling bounds,
/// measured independently of the limiter's own detector.
#[must_use]
pub fn reconstructed_peak(samples: &[f32]) -> f32 {
    const PHASES: u8 = 16;
    const HALF_WIDTH: i16 = 32;
    let mut peak = 0.0_f32;
    // WHY: The trailing window is cut because the signal continues past the buffer in
    // the stream the limiter serves, so a decay to silence there is the test's artefact
    // and not the limiter's output. The leading edge is real and stays measured.
    let tail = usize::from(HALF_WIDTH.unsigned_abs());
    for index in 0..samples.len().saturating_sub(tail) {
        for phase in 0..PHASES {
            let offset = f32::from(phase) / f32::from(PHASES);
            let mut value = 0.0_f32;
            for tap in -HALF_WIDTH..=HALF_WIDTH {
                let Some(&sample) = index
                    .checked_add_signed(isize::from(tap))
                    .and_then(|position| samples.get(position))
                else {
                    continue;
                };
                let distance = offset - f32::from(tap);
                let sinc = if distance.abs() < 1e-6 {
                    1.0
                } else {
                    let argument = PI * distance;
                    argument.sin() / argument
                };
                let window = 0.5 * (1.0 + (PI * distance / f32::from(HALF_WIDTH)).cos()).max(0.0);
                value += sample * sinc * window;
            }
            peak = peak.max(value.abs());
        }
    }
    peak
}
