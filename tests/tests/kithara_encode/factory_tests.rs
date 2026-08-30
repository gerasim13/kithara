use kithara::{self, stream::AudioCodec};
use kithara_encode::{BytesEncodeTarget, EncodeError, EncoderFactory, InnerEncoder};

#[kithara::test]
#[case::aac(AudioCodec::AacLc, 1024)]
#[case::flac(AudioCodec::Flac, 4608)]
fn frame_samples_match_runtime_contract(#[case] codec: AudioCodec, #[case] expected: usize) {
    let frame_samples = EncoderFactory::frame_samples(codec)
        .expect("BUG: codec is in the supported set for this test case");
    assert_eq!(frame_samples, expected);
}

#[kithara::test]
fn frame_samples_reject_unknown_packaged_codec() {
    let error = EncoderFactory::frame_samples(AudioCodec::Mp3)
        .expect_err("BUG: Mp3 is unsupported for packaged encoding");
    assert!(matches!(
        error,
        EncodeError::UnsupportedCodec(AudioCodec::Mp3)
    ));
}

#[kithara::test]
fn create_bytes_returns_public_encoder_abstraction() {
    let _encoder: Box<dyn InnerEncoder> = EncoderFactory::create_bytes(BytesEncodeTarget::Mp3)
        .expect("BUG: mp3 encoder is supported on the test target");
}
