#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    reason = "integration test crate - unwraps are acceptable in test code"
)]

use kithara_test_dylib as _;

mod common {
    pub(crate) use kithara_integration_tests::test_defaults;
}

mod aac_he_v2_hls_decode;
mod aac_priming_regression;
mod apple_mp3_priming_probe;
mod decoder_seek_tests;
mod timeline_tests;
