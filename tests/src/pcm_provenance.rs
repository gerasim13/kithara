pub const SAWTOOTH_PERIOD_FRAMES: usize = 65_536;

const SAWTOOTH_PERIOD_UNITS: i32 = 65_536;
const SAWTOOTH_HALF_PERIOD_UNITS: i32 = 32_768;
const SILENCE_THRESHOLD: f32 = 1.0e-4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameClass {
    Ascending,
    Descending,
    Silence,
    Unknown,
}

/// Per-window classification of a mono f32 stream.
/// `window` = frames per window; `tol` = allowed deviation of the mean
/// per-frame modular delta from +/-1.0 i16 units.
pub fn classify_windows(left: &[f32], window: usize, tol: f32) -> Vec<FrameClass> {
    if window < 2 {
        return Vec::new();
    }

    left.chunks_exact(window)
        .map(|samples| classify_window(samples, tol))
        .collect()
}

/// Sample to sawtooth phase in i16 units, 0..65535.
pub fn phase_units(sample: f32) -> i32 {
    ((sample * 32_768.0).round() as i32 + SAWTOOTH_HALF_PERIOD_UNITS)
        .rem_euclid(SAWTOOTH_PERIOD_UNITS)
}

#[derive(Clone, Copy, Debug)]
pub struct Replay {
    pub start_frame: usize,
    pub len: usize,
    pub start_phase: i32,
}

/// Detect phase-continuity violations inside an ascending sawtooth.
///
/// Silence is a gap the audio did not fill, not audio of its own: an output
/// that underran and resumed carries the track's next frame after the gap, so
/// the expected phase advances with the audio the run actually contains and
/// not with the position in the buffer. A run that resumes further along than
/// the audio between the gaps accounts for is what this reports.
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

    let base_phase = phase_units(left[start]);
    let mut replays = Vec::new();
    let mut active: Option<Replay> = None;
    let mut audio_frames = 0usize;

    for (offset, sample) in left[start..end].iter().copied().enumerate() {
        let frame = start + offset;
        let expected = expected_phase(base_phase, audio_frames);
        let is_violation =
            !is_silence(sample) && modular_delta(expected, phase_units(sample)).abs() > tol_units;
        if !is_gap(sample, expected) {
            audio_frames += 1;
        }

        match (is_violation, active.as_mut()) {
            (true, Some(replay)) => {
                replay.len += 1;
            }
            (true, None) => {
                active = Some(Replay {
                    start_frame: frame,
                    len: 1,
                    start_phase: phase_units(sample),
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

fn classify_window(samples: &[f32], tol: f32) -> FrameClass {
    if samples.iter().all(|sample| is_silence(*sample)) {
        return FrameClass::Silence;
    }

    let delta_sum = samples
        .windows(2)
        .map(|pair| modular_delta(phase_units(pair[0]), phase_units(pair[1])) as f32)
        .sum::<f32>();
    let mean_delta = delta_sum / (samples.len() - 1) as f32;

    if (mean_delta - 1.0).abs() <= tol {
        FrameClass::Ascending
    } else if (mean_delta + 1.0).abs() <= tol {
        FrameClass::Descending
    } else {
        FrameClass::Unknown
    }
}

/// Frames of the track's own audio inside `start..end`.
///
/// An output that underran carries silence the track never played, so a span
/// of the render buffer is longer than the audio it delivered. Callers that
/// measure how much of a track was heard count this rather than the distance
/// between two buffer positions.
pub fn audio_frames(left: &[f32], start: usize, end: usize) -> usize {
    let end = end.min(left.len());
    if start >= end {
        return 0;
    }

    let base_phase = phase_units(left[start]);
    let mut audio = 0usize;
    for sample in left[start..end].iter().copied() {
        if !is_gap(sample, expected_phase(base_phase, audio)) {
            audio += 1;
        }
    }

    audio
}

fn is_gap(sample: f32, expected: i32) -> bool {
    is_silence(sample) && !crosses_zero(expected)
}

fn expected_phase(base_phase: i32, frame_offset: usize) -> i32 {
    let offset = (frame_offset % SAWTOOTH_PERIOD_FRAMES) as i32;
    (base_phase + offset).rem_euclid(SAWTOOTH_PERIOD_UNITS)
}

/// A sawtooth passes through zero once per period, and the frames around that
/// crossing read as silent while being the track's own audio. Only the phase
/// the silence threshold covers qualifies, so the same threshold decides both
/// questions.
fn crosses_zero(expected: i32) -> bool {
    const HALF_WIDTH: i32 = (SILENCE_THRESHOLD * 32_768.0) as i32 + 1;

    (expected - SAWTOOTH_HALF_PERIOD_UNITS).abs() <= HALF_WIDTH
}

const fn modular_delta(current: i32, next: i32) -> i32 {
    (next - current + SAWTOOTH_HALF_PERIOD_UNITS).rem_euclid(SAWTOOTH_PERIOD_UNITS)
        - SAWTOOTH_HALF_PERIOD_UNITS
}

fn is_silence(sample: f32) -> bool {
    sample.abs() < SILENCE_THRESHOLD
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{
        FrameClass, Replay, SAWTOOTH_PERIOD_FRAMES, ascending_phase_replays, classify_windows,
        phase_units,
    };

    const WINDOW: usize = 64;

    #[kithara::test(native, flash(false))]
    fn classify_windows_labels_pure_signals_including_wrap() {
        let ascending = ascending_signal(SAWTOOTH_PERIOD_FRAMES - 32, WINDOW);
        assert_eq!(
            classify_windows(&ascending, WINDOW, 0.5),
            vec![FrameClass::Ascending]
        );

        let descending = descending_signal(SAWTOOTH_PERIOD_FRAMES - 32, WINDOW);
        assert_eq!(
            classify_windows(&descending, WINDOW, 0.5),
            vec![FrameClass::Descending]
        );

        let silence = vec![0.0; WINDOW];
        assert_eq!(
            classify_windows(&silence, WINDOW, 0.5),
            vec![FrameClass::Silence]
        );
    }

    #[kithara::test(native, flash(false))]
    fn phase_units_round_trips_i16_sawtooth_values() {
        assert_eq!(phase_units(i16_to_f32(-32_768)), 0);
        assert_eq!(phase_units(i16_to_f32(-32_767)), 1);
        assert_eq!(phase_units(i16_to_f32(0)), 32_768);
        assert_eq!(phase_units(i16_to_f32(32_767)), 65_535);
    }

    #[kithara::test(native, flash(false))]
    fn ascending_phase_replays_accepts_pure_ascending_run_including_wrap() {
        let left = ascending_signal(SAWTOOTH_PERIOD_FRAMES - 32, WINDOW * 2);

        assert!(ascending_phase_replays(&left, 0, left.len(), 3).is_empty());
    }

    #[kithara::test(native, flash(false))]
    fn ascending_phase_replays_reports_spliced_replay_from_start() {
        let splice_start = 200_000;
        let replay_len = 1_000;
        let total_len = splice_start + replay_len + 2_000;
        let mut left = ascending_signal(0, total_len);
        let replay = ascending_signal(0, replay_len);
        left[splice_start..splice_start + replay_len].copy_from_slice(&replay);

        let replays = ascending_phase_replays(&left, 0, left.len(), 3);

        assert_eq!(replays.len(), 1);
        assert_replay(
            replays[0],
            Replay {
                start_frame: splice_start,
                len: replay_len,
                start_phase: 0,
            },
        );
    }

    #[kithara::test(native, flash(false))]
    fn ascending_phase_replays_reports_descending_region() {
        let descending_start = 128;
        let descending_len = 64;
        let mut left = ascending_signal(0, 512);
        let descending = descending_signal(0, descending_len);
        left[descending_start..descending_start + descending_len].copy_from_slice(&descending);

        let replays = ascending_phase_replays(&left, 0, left.len(), 3);

        assert_eq!(replays.len(), 1);
        assert_replay(
            replays[0],
            Replay {
                start_frame: descending_start,
                len: descending_len,
                start_phase: 65_535,
            },
        );
    }

    #[kithara::test(native, flash(false))]
    fn ascending_phase_replays_reads_through_an_underrun_gap() {
        const HEAD: usize = 256;
        const GAP: usize = 128;

        let mut left = ascending_signal(0, HEAD);
        left.resize(HEAD + GAP, 0.0);
        left.extend(ascending_signal(HEAD, 256));

        assert!(ascending_phase_replays(&left, 0, left.len(), 3).is_empty());
    }

    #[kithara::test(native, flash(false))]
    fn ascending_phase_replays_reports_audio_missing_across_a_gap() {
        const HEAD: usize = 256;
        const GAP: usize = 128;
        const LOST: usize = 64;
        const TAIL: usize = 256;

        let mut left = ascending_signal(0, HEAD);
        left.resize(HEAD + GAP, 0.0);
        left.extend(ascending_signal(HEAD + LOST, TAIL));

        let replays = ascending_phase_replays(&left, 0, left.len(), 3);

        assert_eq!(replays.len(), 1);
        assert_replay(
            replays[0],
            Replay {
                start_frame: HEAD + GAP,
                len: TAIL,
                start_phase: i32::try_from(HEAD + LOST).expect("phase fits i32"),
            },
        );
    }

    fn assert_replay(actual: Replay, expected: Replay) {
        assert_eq!(actual.start_frame, expected.start_frame);
        assert_eq!(actual.len, expected.len);
        assert_eq!(actual.start_phase, expected.start_phase);
    }

    fn ascending_signal(start_frame: usize, len: usize) -> Vec<f32> {
        (start_frame..start_frame + len)
            .map(ascending_sample)
            .collect()
    }

    fn descending_signal(start_frame: usize, len: usize) -> Vec<f32> {
        (start_frame..start_frame + len)
            .map(descending_sample)
            .collect()
    }

    fn ascending_sample(frame: usize) -> f32 {
        let unit = i32::try_from(frame % SAWTOOTH_PERIOD_FRAMES).expect("phase fits i32");
        i16_to_f32(i16::try_from(unit - 32_768).expect("ascending sample fits i16"))
    }

    fn descending_sample(frame: usize) -> f32 {
        let unit = i32::try_from(frame % SAWTOOTH_PERIOD_FRAMES).expect("phase fits i32");
        i16_to_f32(i16::try_from(32_767 - unit).expect("descending sample fits i16"))
    }

    fn i16_to_f32(sample: i16) -> f32 {
        f32::from(sample) / 32_768.0
    }
}
