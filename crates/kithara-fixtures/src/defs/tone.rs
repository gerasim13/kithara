use num_traits::cast;

/// One frame of a sine at `peak`, rounded into 16-bit.
pub(super) fn sine(frame: usize, sample_rate: u32, tone_hz: f64, peak: i16) -> i16 {
    let frame = f64::from(u32::try_from(frame).expect("invariant: a fixture is under 2^32 frames"));
    let phase = std::f64::consts::TAU * tone_hz * frame / f64::from(sample_rate);
    let scaled = (phase.sin() * f64::from(peak))
        .round()
        .clamp(f64::from(i16::MIN), f64::from(i16::MAX));
    cast(scaled).unwrap_or(0)
}
