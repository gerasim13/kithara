use kithara_encode::{BytesEncodeRequest, BytesEncodeTarget, EncoderFactory};
use kithara_test_macros as kithara;

use super::{pcm::Pcm, tone};

struct Consts;

impl Consts {
    const CHANNELS: u16 = 2;
    const ONE_SECOND_FRAMES: usize = 44_100;
    const SAMPLE_RATE: u32 = 44_100;
    const TONE_HZ: f64 = 440.0;
    const TONE_PEAK: i16 = 16_000;
    const TWO_SECOND_FRAMES: usize = 88_200;
}

/// 440 Hz tone encoded to MPEG audio.
#[kithara::asset(ext = "mp3", content_type = "audio/mpeg")]
#[case::a440_2s(Consts::TWO_SECOND_FRAMES, Consts::TONE_PEAK)]
fn sine_mp3(total_frames: usize, peak: i16) -> Vec<u8> {
    let pcm = Pcm::new(
        Consts::SAMPLE_RATE,
        Consts::CHANNELS,
        total_frames,
        |frame| tone::sine(frame, Consts::SAMPLE_RATE, Consts::TONE_HZ, peak),
    );
    EncoderFactory::encode_bytes(BytesEncodeRequest {
        pcm: &pcm,
        target: BytesEncodeTarget::Mp3,
        bit_rate: None,
    })
    .unwrap_or_else(|error| panic!("kithara-test-fixtures: MP3 encode failed: {error}"))
    .bytes
}

/// Silence encoded losslessly into an MP4 container: the standalone ALAC body
/// the Apple `AudioFileServices` path must decode.
#[kithara::asset(ext = "m4a", content_type = "audio/mp4")]
#[case::silence_1s(Consts::ONE_SECOND_FRAMES)]
fn alac(total_frames: usize) -> Vec<u8> {
    let pcm = Pcm::new(
        Consts::SAMPLE_RATE,
        Consts::CHANNELS,
        total_frames,
        |_frame| 0,
    );
    EncoderFactory::encode_bytes(BytesEncodeRequest {
        pcm: &pcm,
        target: BytesEncodeTarget::Alac,
        bit_rate: None,
    })
    .unwrap_or_else(|error| panic!("kithara-test-fixtures: ALAC encode failed: {error}"))
    .bytes
}
