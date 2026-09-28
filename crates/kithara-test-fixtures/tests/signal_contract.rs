#![cfg(not(target_arch = "wasm32"))]

//! The signal primitives read against the prepared inputs the build wrote: a
//! detector is only as good as what it says about a stored signal.

#[cfg(all(test, target_os = "android"))]
use kithara_test_dylib as _;
use kithara_test_fixtures::{
    fixtures::{
        ascending_pcm, ascending_wrap_pcm, descending_pcm, descending_wrap_pcm,
        direction_channel_less, direction_step, phase_endpoints, provenance_silence,
    },
    integration_fixtures::encoder_saw_aac,
    signal::{
        FrameClass, Pcm, Replay, SAW_PERIOD, SignalDirection, ascending_phase_replays,
        classify_windows, detect_direction, phase::units,
    },
};
use kithara_test_utils::kithara;

mod consts {
    pub(super) const WINDOW: usize = 64;
}

#[kithara::test(native)]
fn prepared_bytes_preserve_pcm_and_reject_incomplete_frames(encoder_saw_aac: Pcm) {
    let bytes = Vec::from(encoder_saw_aac);
    let pcm = Pcm::from((48_000, 2, bytes.clone()));
    assert_eq!(pcm.sample_rate(), 48_000);
    assert_eq!(pcm.channels(), 2);
    assert_eq!(Vec::from(pcm), bytes);
    for (rate, channels, trim) in [(0, 2, 0), (48_000, 0, 0), (48_000, 2, 1)] {
        let invalid = bytes[..bytes.len() - trim].to_vec();
        assert!(std::panic::catch_unwind(|| Pcm::from((rate, channels, invalid))).is_err());
    }
}

#[kithara::test(native)]
fn units_round_trip_the_i16_sawtooth_range(phase_endpoints: Vec<f32>) {
    assert_eq!(units(phase_endpoints[0]), 0);
    assert_eq!(units(phase_endpoints[1]), 1);
    assert_eq!(units(phase_endpoints[2]), 32_768);
    assert_eq!(units(phase_endpoints[3]), SAW_PERIOD - 1);
}

#[kithara::test(native)]
fn one_ascending_step_per_frame_reads_as_ascending(direction_step: Vec<f32>) {
    assert_eq!(
        detect_direction(&direction_step, 2),
        SignalDirection::Ascending
    );
}

#[kithara::test(native)]
fn a_channel_less_buffer_has_no_direction(direction_channel_less: Vec<f32>) {
    assert_eq!(
        detect_direction(&direction_channel_less, 0),
        SignalDirection::Unknown
    );
}

#[kithara::test(native)]
fn classify_windows_labels_pure_signals_including_wrap(
    ascending_wrap_pcm: Vec<f32>,
    descending_wrap_pcm: Vec<f32>,
    provenance_silence: Vec<f32>,
) {
    let ascending = &ascending_wrap_pcm[..consts::WINDOW];
    assert_eq!(
        classify_windows(ascending, consts::WINDOW, 0.5),
        vec![FrameClass::Ascending]
    );

    let descending = descending_wrap_pcm;
    assert_eq!(
        classify_windows(&descending, consts::WINDOW, 0.5),
        vec![FrameClass::Descending]
    );

    let silence = provenance_silence;
    assert_eq!(
        classify_windows(&silence, consts::WINDOW, 0.5),
        vec![FrameClass::Silence]
    );
}

/// A splice is a splice wherever it falls. The frame it lands on is set by
/// whatever the renderer had committed when the flush arrived, so a reader
/// that saw only the steps inside a window would report the same stream as
/// continuous in one run and broken in the next.
#[kithara::test(native)]
fn a_splice_on_a_window_boundary_still_breaks_the_class(ascending_pcm: Vec<f32>) {
    const JUMP: usize = SAW_PERIOD / 2;

    let mut left = ascending_pcm[..consts::WINDOW * 2].to_vec();
    left[consts::WINDOW..].copy_from_slice(&ascending_pcm[JUMP..JUMP + consts::WINDOW]);

    assert_eq!(
        classify_windows(&left, consts::WINDOW, 0.5),
        vec![FrameClass::Ascending, FrameClass::Unknown]
    );
}

#[kithara::test(native)]
fn ascending_phase_replays_accepts_pure_ascending_run_including_wrap(ascending_wrap_pcm: Vec<f32>) {
    let left = ascending_wrap_pcm;

    assert!(ascending_phase_replays(&left, 0, left.len(), 3).is_empty());
}

#[kithara::test(native)]
fn ascending_phase_replays_reports_spliced_replay_from_start(ascending_pcm: Vec<f32>) {
    let splice_start = 200_000;
    let replay_len = 1_000;
    let total_len = splice_start + replay_len + 2_000;
    let mut left = ascending_pcm[..total_len].to_vec();
    let replay = &ascending_pcm[..replay_len];
    left[splice_start..splice_start + replay_len].copy_from_slice(replay);

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

#[kithara::test(native)]
fn ascending_phase_replays_reports_descending_region(
    ascending_pcm: Vec<f32>,
    descending_pcm: Vec<f32>,
) {
    let descending_start = 128;
    let descending_len = 64;
    let mut left = ascending_pcm[..512].to_vec();
    let descending = descending_pcm;
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

fn assert_replay(actual: Replay, expected: Replay) {
    assert_eq!(actual.start_frame, expected.start_frame);
    assert_eq!(actual.len, expected.len);
    assert_eq!(actual.start_phase, expected.start_phase);
}
