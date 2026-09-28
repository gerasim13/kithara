use num_traits::cast;

use super::{SAW_PERIOD, phase};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameClass {
    Ascending,
    Descending,
    Silence,
    Unknown,
}

/// Per-window classification of a mono f32 stream.
///
/// `window` = frames per window; `tol` = allowed deviation of the mean
/// per-frame modular delta from +/-1.0 i16 units.
///
/// A window's mean covers the step into its first frame as well as the steps
/// between its own frames, so the windows together read every step the stream
/// contains. Reading only the interior steps would drop one step per window --
/// precisely the steps that land on a window boundary -- and a splice that
/// landed there would be invisible to every caller.
#[must_use]
pub fn classify_windows(left: &[f32], window: usize, tol: f32) -> Vec<FrameClass> {
    if window < 2 {
        return Vec::new();
    }

    left.chunks_exact(window)
        .enumerate()
        .map(|(index, samples)| {
            let preceding = (index > 0).then(|| left[index * window - 1]);
            classify_window(samples, preceding, tol)
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
pub struct Replay {
    pub len: usize,
    pub start_frame: usize,
    pub start_phase: usize,
}

/// Detect phase-continuity violations inside a contiguous ascending sawtooth.
#[must_use]
pub fn ascending_phase_replays(
    left: &[f32],
    start: usize,
    end: usize,
    tol_units: i32,
) -> Vec<Replay> {
    let end = end.min(left.len());
    if start >= end {
        return Vec::new();
    }

    let base_phase = phase::units(left[start]);
    let mut replays = Vec::new();
    let mut active: Option<Replay> = None;

    for (offset, sample) in left[start..end].iter().copied().enumerate() {
        let frame = start + offset;
        let is_violation = !is_silence(sample)
            && expected_phase_error(sample, base_phase, offset).abs() > tol_units;

        match (is_violation, active.as_mut()) {
            (true, Some(replay)) => {
                replay.len += 1;
            }
            (true, None) => {
                active = Some(Replay {
                    start_frame: frame,
                    len: 1,
                    start_phase: phase::units(sample),
                });
            }
            (false, Some(_)) => {
                if let Some(replay) = active.take() {
                    replays.push(replay);
                }
            }
            (false, None) => {}
        }
    }

    if let Some(replay) = active {
        replays.push(replay);
    }

    replays
}

fn classify_window(samples: &[f32], preceding: Option<f32>, tol: f32) -> FrameClass {
    if samples.iter().all(|sample| is_silence(*sample)) {
        return FrameClass::Silence;
    }

    let entry = preceding.map(|prior| step(prior, samples[0]));
    let delta_sum = entry
        .into_iter()
        .chain(samples.windows(2).map(|pair| step(pair[0], pair[1])))
        .sum::<f32>();
    let count = samples.len() - 1 + usize::from(entry.is_some());
    let steps: f32 = cast(count).expect("invariant: a window length fits f32");
    let mean_delta = delta_sum / steps;

    if (mean_delta - 1.0).abs() <= tol {
        FrameClass::Ascending
    } else if (mean_delta + 1.0).abs() <= tol {
        FrameClass::Descending
    } else {
        FrameClass::Unknown
    }
}

fn step(from: f32, to: f32) -> f32 {
    f32::from(phase::delta(phase::units(from), phase::units(to)))
}

fn expected_phase_error(sample: f32, base_phase: usize, frame_offset: usize) -> i32 {
    let expected = (base_phase + frame_offset % SAW_PERIOD) % SAW_PERIOD;
    i32::from(phase::delta(expected, phase::units(sample)))
}

fn is_silence(sample: f32) -> bool {
    const SILENCE_THRESHOLD: f32 = 1.0e-4;
    sample.abs() < SILENCE_THRESHOLD
}
