#![cfg(all(feature = "native-fixtures", not(target_arch = "wasm32")))]

#[cfg(all(test, target_os = "android"))]
use kithara_test_dylib as _;
use kithara_test_fixtures::{asset::Asset, assets, unit_fixtures};
use kithara_test_utils::kithara;

#[kithara::test(native)]
#[case::chirp(true)]
#[case::sine(false)]
fn prepared_render_signal_preserves_every_host_sample_bit(#[case] chirp: bool) {
    let (prepared, source) = if chirp {
        (unit_fixtures::render_chirp(), assets::render_signal_chirp())
    } else {
        (unit_fixtures::render_sine(), assets::render_signal_sine())
    };
    assert_sample_bits(&prepared, source.bytes(), 49_152);
}

#[kithara::test(native)]
#[case::outgoing(false)]
#[case::incoming(true)]
fn prepared_host_pcm_and_wav_preserve_every_prepared_byte(#[case] incoming: bool) {
    let (prepared, source, asset) = if incoming {
        (
            unit_fixtures::host_manual_incoming(),
            assets::host_manual_pcm_incoming(),
            assets::host_manual_wav_incoming(),
        )
    } else {
        (
            unit_fixtures::host_manual_outgoing(),
            assets::host_manual_pcm_outgoing(),
            assets::host_manual_wav_outgoing(),
        )
    };
    let pcm = source.bytes();
    assert_sample_bits(&prepared, pcm, 529_200);

    let data_bytes = u32::try_from(pcm.len()).expect("WAV data size");
    let mut expected = Vec::new();
    expected.extend_from_slice(b"RIFF");
    expected.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    expected.extend_from_slice(b"WAVEfmt ");
    expected.extend_from_slice(&16_u32.to_le_bytes());
    expected.extend_from_slice(&3_u16.to_le_bytes());
    expected.extend_from_slice(&2_u16.to_le_bytes());
    expected.extend_from_slice(&44_100_u32.to_le_bytes());
    expected.extend_from_slice(&352_800_u32.to_le_bytes());
    expected.extend_from_slice(&8_u16.to_le_bytes());
    expected.extend_from_slice(&32_u16.to_le_bytes());
    expected.extend_from_slice(b"data");
    expected.extend_from_slice(&data_bytes.to_le_bytes());
    expected.extend_from_slice(pcm);
    assert_eq!(expected.len(), 4_233_644);
    assert_eq!(asset.bytes(), expected);
    assert_eq!(
        std::fs::read(asset.path().expect("prepared WAV path")).expect("prepared WAV bytes"),
        expected
    );
}

#[kithara::test(native)]
fn prepared_stretch_kick_body_preserves_every_host_sample_bit() {
    assert_sample_bits(
        &unit_fixtures::stretch_kick_body(),
        assets::stretch_kick_body_default().bytes(),
        292_800,
    );
}

#[kithara::test(native)]
fn prepared_warp_tone_preserves_every_host_sample_bit() {
    assert_sample_bits(
        &unit_fixtures::warp_native_tone(),
        assets::warp_native_tone_default().bytes(),
        8_192,
    );
}

#[kithara::test(native)]
#[case::held(
    unit_fixtures::host_metronome_held,
    assets::host_metronome_held_default,
    8_192
)]
#[case::tone(
    unit_fixtures::host_metronome_tone,
    assets::host_metronome_tone_default,
    8_192
)]
fn prepared_metronome_mixes_preserve_every_host_sample_and_file_byte(
    #[case] read: fn() -> Vec<f32>,
    #[case] source: fn() -> Asset,
    #[case] frames: usize,
) {
    let prepared = read();
    let source = source();
    assert_eq!(prepared.len(), frames);
    assert_prepared_bits(&prepared, source.bytes());
    assert_file_bytes(&source);
}

#[kithara::test(native)]
#[case::outgoing(
    unit_fixtures::render_crossfade_outgoing,
    assets::render_crossfade_pcm_outgoing,
    177_524
)]
#[case::incoming(
    unit_fixtures::render_crossfade_incoming,
    assets::render_crossfade_pcm_incoming,
    177_524
)]
#[case::readiness(
    unit_fixtures::render_readiness_pcm,
    assets::render_readiness_pcm_default,
    4_096
)]
#[case::before_seek(
    unit_fixtures::render_seek_before,
    assets::render_seek_pcm_before,
    1_024
)]
#[case::after_seek(unit_fixtures::render_seek_after, assets::render_seek_pcm_after, 1_024)]
fn prepared_render_sources_preserve_every_host_sample_and_file_byte(
    #[case] read: fn() -> Vec<f32>,
    #[case] source: fn() -> Asset,
    #[case] frames: usize,
) {
    let source = source();
    assert_sample_bits(&read(), source.bytes(), frames);
    assert_file_bytes(&source);
}

#[kithara::test(native)]
fn prepared_readiness_source_preserves_negative_zero() {
    assert_eq!(
        unit_fixtures::render_readiness_pcm()[1].to_bits(),
        (-0.0_f32).to_bits()
    );
}

#[cfg(feature = "encoded")]
#[kithara::test(native)]
fn prepared_opus_packet_and_pcm_preserve_every_host_file_byte() {
    let pcm = assets::decoder_opus_pcm_default();
    assert_eq!(pcm.bytes().len(), 960 * 2 * size_of::<f32>());
    assert_file_bytes(&pcm);
    let packet = assets::decoder_opus_packet_default();
    assert!(!packet.bytes().is_empty());
    assert!(packet.bytes().len() <= 4_000);
    assert_eq!(unit_fixtures::decoder_opus_packet(), packet.bytes());
    assert_file_bytes(&packet);
}

fn assert_file_bytes(source: &Asset) {
    assert_eq!(
        std::fs::read(source.path().expect("prepared source path")).expect("prepared source bytes"),
        source.bytes(),
    );
}

fn assert_sample_bits(prepared: &[f32], bytes: &[u8], frames: usize) {
    assert_eq!(prepared.len(), frames * 2);
    assert_prepared_bits(prepared, bytes);
}

fn assert_prepared_bits(prepared: &[f32], bytes: &[u8]) {
    assert_eq!(bytes.len(), size_of_val(prepared));
    for (index, (actual, expected)) in prepared.iter().zip(bytes.as_chunks::<4>().0).enumerate() {
        assert_eq!(actual.to_le_bytes(), *expected, "sample {index}");
    }
}
