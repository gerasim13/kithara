use num_traits::ToPrimitive;

/// Full-depth duck input rising to the given limiter ceiling over 512 frames.
#[must_use]
pub fn metronome_held(ceiling: f32) -> Vec<f32> {
    (0..8_192_u16)
        .map(|frame| {
            let rise = (f32::from(frame) / 512.0).min(1.0);
            ceiling * 0.5 * (1.0 - (std::f32::consts::PI * rise).cos())
        })
        .collect()
}

/// A 10 kHz moving mix at 44.1 kHz under the supplied duck envelope.
///
/// # Panics
///
/// Panics if a modulated sample cannot be represented as `f32`.
#[must_use]
pub fn metronome_tone(held: &[f32]) -> Vec<f32> {
    held.iter()
        .zip(0..8_192_u16)
        .map(|(level, frame)| {
            let cycles = (f64::from(frame) * 10_000.0 / f64::from(44_100)).fract();
            let tone = f64::from(*level) * std::f64::consts::TAU.mul_add(cycles, 2.1).cos();
            tone.to_f32()
                .expect("a bounded f32 level times a cosine fits f32")
        })
        .collect()
}
