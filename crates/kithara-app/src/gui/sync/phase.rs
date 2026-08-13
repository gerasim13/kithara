use std::f64::consts::TAU;

use num_traits::{ToPrimitive, cast::AsPrimitive};

pub(super) fn phase_distance(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    let stagger = (a? - b?).abs();
    let phase = stagger.fract();
    Some(phase.min(1.0 - phase))
}

pub(super) fn circular_phase(frames: &[i64], period: u64) -> Option<(u64, f64)> {
    if frames.is_empty() || period == 0 {
        return None;
    }
    let period_i64 = i64::try_from(period).ok()?;
    let period_f64 = period.to_f64()?;
    let (sin_sum, cos_sum) = frames.iter().fold((0.0_f64, 0.0_f64), |(sin, cos), frame| {
        let remainder: f64 = frame.rem_euclid(period_i64).as_();
        let angle = remainder / period_f64 * TAU;
        (sin + angle.sin(), cos + angle.cos())
    });
    let concentration = sin_sum.hypot(cos_sum) / frames.len().to_f64()?;
    let angle = sin_sum.atan2(cos_sum).rem_euclid(TAU);
    let phase = (angle / TAU * period_f64).round().to_u64()? % period;
    Some((phase, concentration))
}

pub(super) fn circular_spread(phases: &[u64], period: u64) -> Option<u64> {
    if phases.len() < 2 || period == 0 {
        return None;
    }
    let mut phases = phases.to_vec();
    phases.sort_unstable();
    let largest_gap = phases
        .windows(2)
        .map(|window| window[1] - window[0])
        .chain(std::iter::once(
            period - phases[phases.len() - 1] + phases[0],
        ))
        .max()?;
    Some(period - largest_gap)
}
