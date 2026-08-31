use kithara_integration_tests::{cochlea::synchronization_failures, kithara};
use kithara_test_fixtures::{
    asset::Asset,
    assets::{
        rhythm_wav_deck_a_120bpm_48k, rhythm_wav_deck_b_120bpm_48k,
        rhythm_wav_deck_b_missing_beat_120bpm_48k, rhythm_wav_deck_b_one_beat_bar_late_120bpm_48k,
        rhythm_wav_deck_b_one_frame_late_120bpm_48k, rhythm_wav_deck_c_120bpm_48k,
        rhythm_wav_deck_d_120bpm_48k,
    },
};

const CHANNELS: u16 = 2;
const SAMPLE_RATE: u32 = 48_000;
const TARGET_BPM: f64 = 120.0;

fn samples(asset: Asset) -> Vec<f32> {
    let bytes = asset.bytes();
    let header = kithara_test_fixtures::signal::header(SAMPLE_RATE, CHANNELS, Some(0));
    bytes[header.len()..]
        .chunks_exact(size_of::<i16>())
        .map(|sample| f32::from(i16::from_le_bytes([sample[0], sample[1]])) / f32::from(i16::MAX))
        .collect()
}

#[kithara::test(native, flash(false))]
fn static_rhythmic_oracle_accepts_aligned_stems_and_rejects_one_frame_phase_error() {
    let deck_a = samples(rhythm_wav_deck_a_120bpm_48k());
    let deck_b = samples(rhythm_wav_deck_b_120bpm_48k());
    let deck_c = samples(rhythm_wav_deck_c_120bpm_48k());
    let deck_d = samples(rhythm_wav_deck_d_120bpm_48k());
    let aligned = [
        deck_a.as_slice(),
        deck_b.as_slice(),
        deck_c.as_slice(),
        deck_d.as_slice(),
    ];

    assert!(
        synchronization_failures(
            "aligned static stems",
            &aligned,
            CHANNELS,
            SAMPLE_RATE,
            TARGET_BPM,
        )
        .is_empty(),
        "known-aligned build-time stems must pass the synchronization oracle",
    );

    let shifted = samples(rhythm_wav_deck_b_one_frame_late_120bpm_48k());
    let one_frame_late = [
        deck_a.as_slice(),
        shifted.as_slice(),
        deck_c.as_slice(),
        deck_d.as_slice(),
    ];
    let failures = synchronization_failures(
        "one-frame-late static stem",
        &one_frame_late,
        CHANNELS,
        SAMPLE_RATE,
        TARGET_BPM,
    );

    assert_eq!(
        failures.as_slice(),
        ["one-frame-late static stem: beat phase spread is 1 frame"],
        "the negative control must fail only because of its exact one-frame phase error",
    );
}

#[kithara::test(native, flash(false))]
fn static_rhythmic_oracle_rejects_one_missing_event_for_that_reason() {
    let deck_a = samples(rhythm_wav_deck_a_120bpm_48k());
    let missing = samples(rhythm_wav_deck_b_missing_beat_120bpm_48k());
    let failures = synchronization_failures(
        "missing static beat",
        &[deck_a.as_slice(), missing.as_slice()],
        CHANNELS,
        SAMPLE_RATE,
        TARGET_BPM,
    );

    assert_eq!(
        failures.as_slice(),
        ["missing static beat: track 1 is missing a rhythmic event before frame 168000"],
    );
}

#[kithara::test(native, flash(false))]
fn static_rhythmic_oracle_rejects_wrong_target_tempo_for_that_reason() {
    let deck = samples(rhythm_wav_deck_a_120bpm_48k());
    let failures = synchronization_failures(
        "wrong target tempo",
        &[deck.as_slice()],
        CHANNELS,
        SAMPLE_RATE,
        127.0,
    );

    assert_eq!(failures.len(), 1, "unexpected failures: {failures:?}");
    assert!(
        failures[0].contains("tempo is") && failures[0].contains("expected 127.000"),
        "the control must fail only on its 120 BPM signal against 127 BPM target: {failures:?}",
    );
}

#[kithara::test(native, flash(false))]
fn static_rhythmic_oracle_rejects_one_beat_bar_phase_error_for_that_reason() {
    let deck_a = samples(rhythm_wav_deck_a_120bpm_48k());
    let shifted = samples(rhythm_wav_deck_b_one_beat_bar_late_120bpm_48k());
    let failures = synchronization_failures(
        "one-beat-late static bar",
        &[deck_a.as_slice(), shifted.as_slice()],
        CHANNELS,
        SAMPLE_RATE,
        TARGET_BPM,
    );

    assert_eq!(
        failures.as_slice(),
        ["one-beat-late static bar: bar phase spread is 1 beat"],
    );
}
