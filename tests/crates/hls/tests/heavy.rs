#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    reason = "integration test crate - unwraps are acceptable in test code"
)]

use kithara_test_dylib as _;

mod common {
    pub(crate) use kithara_integration_tests::test_defaults;
}

mod drm_stream_integrity;
mod hls_abr_variant_switch;
mod stress_chunk_integrity;
mod stress_seek_random;
