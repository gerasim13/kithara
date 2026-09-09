use kithara_stream::AudioCodec;
use kithara_test_fixtures::unit_fixtures::encode_saw_i16;
use kithara_test_utils::kithara;

use super::packaged::OfflineEncoder;
use crate::{BytesEncodeRequest, BytesEncodeTarget, EncodeError, test_pcm::TestPcm};

#[kithara::test(native, flash(false))]
fn byte_encoding_reports_the_missing_backend(encode_saw_i16: &'static [u8]) {
    let pcm = TestPcm::from_bytes(encode_saw_i16[..1024 * 2 * 2].to_vec(), 48_000, 2);

    let error = OfflineEncoder::encode_bytes(&BytesEncodeRequest {
        pcm: &pcm,
        target: BytesEncodeTarget::Mp3,
        bit_rate: None,
    })
    .map(|_| ())
    .expect_err("no FFmpeg, no byte encoding");

    assert!(matches!(error, EncodeError::InvalidInput(_)), "{error}");
}

#[test]
fn a_codec_whose_backend_is_absent_is_unsupported() {
    let error =
        OfflineEncoder::packaged_frame_samples(AudioCodec::Flac).expect_err("no FFmpeg, no FLAC");

    assert!(matches!(error, EncodeError::UnsupportedCodec(_)), "{error}");
}
