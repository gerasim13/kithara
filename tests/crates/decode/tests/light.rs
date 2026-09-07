#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    reason = "integration test crate - unwraps are acceptable in test code"
)]

mod common {
    pub(crate) use kithara_integration_tests::test_defaults;
}

#[path = "decode/aac_priming_regression.rs"]
mod aac_priming_regression;
#[path = "decode/apple_mp3_priming_probe.rs"]
mod apple_mp3_priming_probe;
#[path = "decode/decoder_seek_tests.rs"]
mod decoder_seek_tests;
#[path = "decode/decoder_tests.rs"]
mod decoder_tests;
#[path = "decode/factory_tests.rs"]
mod factory_tests;
#[path = "decode/protocol_tests.rs"]
mod protocol_tests;
#[path = "decode/symphonia_seek_stale_duration.rs"]
mod symphonia_seek_stale_duration;
#[path = "decode/symphonia_tests.rs"]
mod symphonia_tests;
#[path = "decode/timeline_tests.rs"]
mod timeline_tests;
