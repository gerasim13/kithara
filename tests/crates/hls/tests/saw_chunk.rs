//! Checks the HLS stress suites run over one decoded saw-tooth chunk.

use kithara_test_fixtures::signal;

/// The first sample outside a finite `[-1, 1]`, with its offset.
pub(crate) fn first_invalid_sample(samples: &[f32]) -> Option<(usize, f32)> {
    samples
        .iter()
        .copied()
        .enumerate()
        .find(|&(_, sample)| !(sample.is_finite() && (-1.0..=1.0).contains(&sample)))
}

/// Stereo frames whose two channels differ, as `(frame, left, right)`.
pub(crate) fn channel_mismatches(samples: &[f32]) -> impl Iterator<Item = (usize, f32, f32)> + '_ {
    samples
        .chunks_exact(2)
        .enumerate()
        .filter(|(_, frame)| (frame[0] - frame[1]).abs() > f32::EPSILON)
        .map(|(f, frame)| (f, frame[0], frame[1]))
}

/// Each frame's saw phase beside the previous frame's, read from the first
/// channel, as `(frame, previous, current)`.
pub(crate) fn phase_steps(samples: &[f32], channels: usize) -> Vec<(usize, usize, usize)> {
    let phases: Vec<usize> = samples
        .chunks_exact(channels)
        .map(|frame| signal::phase::units(frame[0]))
        .collect();
    phases
        .windows(2)
        .enumerate()
        .map(|(f, pair)| (f + 1, pair[0], pair[1]))
        .collect()
}
