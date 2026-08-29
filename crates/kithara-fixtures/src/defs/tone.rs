use num_traits::cast;

/// Period of the saw-tooth in frames: one full 16-bit sweep.
const SAW_PERIOD: usize = 65_536;

/// One frame of a sine at `peak`, rounded into 16-bit.
pub(super) fn sine(frame: usize, sample_rate: u32, tone_hz: f64, peak: i16) -> i16 {
    let frame = f64::from(u32::try_from(frame).expect("invariant: a fixture is under 2^32 frames"));
    let phase = std::f64::consts::TAU * tone_hz * frame / f64::from(sample_rate);
    let scaled = (phase.sin() * f64::from(peak))
        .round()
        .clamp(f64::from(i16::MIN), f64::from(i16::MAX));
    cast(scaled).unwrap_or(0)
}

/// One frame of an ascending saw-tooth: frame 0 is `i16::MIN`, frame 65535 is
/// `i16::MAX`.
pub(super) fn sawtooth(frame: usize) -> i16 {
    let step = i32::try_from(frame % SAW_PERIOD).expect("invariant: a saw period fits i32");
    let value = step + i32::from(i16::MIN);
    cast(value).expect("invariant: one saw period spans exactly the 16-bit range")
}
