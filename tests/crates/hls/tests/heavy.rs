#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    reason = "integration test crate - unwraps are acceptable in test code"
)]

mod common {
    pub(crate) use kithara_integration_tests::test_defaults;
}

#[path = "hls/deferred_abr_debug.rs"]
mod deferred_abr_debug;
#[path = "hls/drm_stream_integrity.rs"]
mod drm_stream_integrity;
#[path = "hls/stress_chunk_integrity.rs"]
mod stress_chunk_integrity;
#[path = "hls/stress_seek_random.rs"]
mod stress_seek_random;
