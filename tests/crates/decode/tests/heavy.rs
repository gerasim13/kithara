#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    reason = "integration test crate - unwraps are acceptable in test code"
)]

mod common {
    pub(crate) use kithara_integration_tests::test_defaults;
}
pub use kithara_integration_tests::gapless as gapless_common;

#[path = "decode/fixture_integration.rs"]
mod fixture_integration;
#[path = "decode/gapless_encoding_parity.rs"]
mod gapless_encoding_parity;
#[path = "decode/gapless_parity.rs"]
mod gapless_parity;
#[path = "decode/hls_abr_variant_switch.rs"]
mod hls_abr_variant_switch;
#[path = "decode/stress_seek_random.rs"]
mod stress_seek_random;
#[path = "decode/stress_timeline.rs"]
mod stress_timeline;
