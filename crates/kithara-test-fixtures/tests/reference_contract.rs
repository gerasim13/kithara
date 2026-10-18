#![cfg(not(target_arch = "wasm32"))]

#[cfg(target_os = "android")]
use kithara_test_dylib as _;
use kithara_test_fixtures::{assets::by_name, reference};
use kithara_test_utils::kithara;

#[kithara::test(native)]
fn prepared_reference_retains_the_full_native_wav() {
    let asset = by_name("rhythm_wav_deck_a_120bpm_48k").expect("whole-track fixture");
    let reference = reference::audio(&asset).expect("independent prepared PCM");
    assert_eq!((reference.sample_rate, reference.channels), (48_000, 2));
    assert_eq!(reference.samples.len(), 48_000 * 12 * 2);
    assert_eq!(
        reference.presentation_frames.map(std::num::NonZeroU64::get),
        Some(48_000 * 12)
    );
    assert!(reference.samples.iter().any(|sample| *sample != 0.0));
    let wav = asset.bytes();
    let expected = wav[44..].as_chunks::<2>().0;
    assert_eq!(reference.samples.len(), expected.len());
    assert!(
        reference
            .samples
            .iter()
            .zip(expected)
            .all(|(actual, expected)| {
                *actual == f32::from(i16::from_le_bytes(*expected)) / 32_768.0
            })
    );
    let repeated = reference::audio(&asset).expect("repeat prepared read");
    assert_eq!(reference, repeated);
}

#[kithara::test(native)]
fn a_non_audio_asset_has_no_generated_reference() {
    let asset = by_name("analysis_rhythm_wav_deck_a_120bpm_48k").expect("declared analysis");
    let error = reference::audio(&asset).expect_err("an analysis is not an audio track");
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    assert_eq!(
        reference::source(&asset)
            .expect_err("an analysis is not source PCM")
            .kind(),
        std::io::ErrorKind::NotFound,
    );
}

#[kithara::test(native)]
fn prepared_native_source_retains_the_entire_decoder_input() {
    let asset = by_name("rhythm_wav_deck_a_120bpm_48k").expect("whole-track fixture");
    let source = reference::source(&asset).expect("prepared native analysis input");
    assert_eq!((source.sample_rate, source.channels), (48_000, 2));
    assert_eq!(source.presentation_frames, None);
    let pcm = asset.bytes()[44..].as_chunks::<2>().0;
    assert_eq!(source.samples.len(), 48_000 * 12 * 2);
    assert_eq!(source.samples.len(), pcm.len());
    assert!(source.samples.iter().zip(pcm).all(|(actual, expected)| {
        actual.to_bits() == (f32::from(i16::from_le_bytes(*expected)) / 32_768.0).to_bits()
    }));
}

#[kithara::test(native)]
#[case::deck_a("deck_a_120bpm_48k")]
#[case::deck_b("deck_b_120bpm_48k")]
#[case::deck_c("deck_c_120bpm_48k")]
#[case::deck_d("deck_d_120bpm_48k")]
#[case::one_frame("deck_b_one_frame_late_120bpm_48k")]
#[case::one_bar("deck_b_one_beat_bar_late_120bpm_48k")]
#[case::missing("deck_b_missing_beat_120bpm_48k")]
#[case::dub("ambient_dub_62_aligned")]
#[case::dub_frame("ambient_dub_62_one_frame_late")]
#[case::dub_bar("ambient_dub_62_one_beat_bar_late")]
#[case::dub_missing("ambient_dub_62_missing_beat")]
#[case::trip("trip_hop_74_aligned")]
#[case::trip_frame("trip_hop_74_one_frame_late")]
#[case::trip_bar("trip_hop_74_one_beat_bar_late")]
#[case::trip_missing("trip_hop_74_missing_beat")]
#[case::down("downtempo_96_aligned")]
#[case::down_frame("downtempo_96_one_frame_late")]
#[case::down_bar("downtempo_96_one_beat_bar_late")]
#[case::down_missing("downtempo_96_missing_beat")]
#[case::house("house_124_aligned")]
#[case::house_frame("house_124_one_frame_late")]
#[case::house_bar("house_124_one_beat_bar_late")]
#[case::house_missing("house_124_missing_beat")]
#[case::techno("techno_132_aligned")]
#[case::techno_frame("techno_132_one_frame_late")]
#[case::techno_bar("techno_132_one_beat_bar_late")]
#[case::techno_missing("techno_132_missing_beat")]
#[case::breakbeat("breakbeat_140_aligned")]
#[case::break_frame("breakbeat_140_one_frame_late")]
#[case::break_bar("breakbeat_140_one_beat_bar_late")]
#[case::break_missing("breakbeat_140_missing_beat")]
fn authored_reference_preserves_every_original_sample_bit(#[case] suffix: &str) {
    let asset = by_name(&format!("rhythm_wav_{suffix}")).expect("authored WAV fixture");
    let reference = reference::authored(&asset).expect("prepared authored PCM");
    let (pcm, remainder) = asset.bytes()[44..].as_chunks::<2>();
    assert!(remainder.is_empty());
    assert_eq!((reference.sample_rate, reference.channels), (48_000, 2));
    assert_eq!(reference.samples.len(), pcm.len());
    assert_eq!(
        reference.presentation_frames.map(std::num::NonZeroU64::get),
        Some(576_000)
    );
    assert!(reference.samples.iter().zip(pcm).all(|(actual, sample)| {
        actual.to_bits() == (f32::from(i16::from_le_bytes(*sample)) / f32::from(i16::MAX)).to_bits()
    }));
}

#[kithara::test(native)]
fn prepared_mp3_source_window_keeps_the_original_wav_extent_and_bits() {
    let asset = by_name("rhythm_wav_deck_b_120bpm_48k").expect("MP3 source WAV");
    let reference = reference::mp3_clock_window();
    assert_eq!(reference.samples.len(), 144_000 * 2);
    assert_eq!(
        reference.presentation_frames.map(std::num::NonZeroU64::get),
        Some(144_000)
    );
    let pcm = asset.bytes()[44..].as_chunks::<2>().0;
    assert!(reference.samples.iter().zip(pcm).all(|(actual, sample)| {
        actual.to_bits() == (f32::from(i16::from_le_bytes(*sample)) / f32::from(i16::MAX)).to_bits()
    }));
    assert_eq!(
        reference::mp3_clock_markers(),
        (
            vec![24_000, 48_000, 72_000, 96_000, 120_000],
            vec![24_000, 120_000],
        )
    );
}
