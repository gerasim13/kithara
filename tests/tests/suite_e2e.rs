#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    reason = "integration test crate — unwraps are acceptable in test code"
)]
//! Playback against real audio hardware. Everything here stays inside the
//! runner: the fixtures are local, and the tests that need a remote host live
//! in `suite_network` behind the `network` feature.

#[cfg(not(target_arch = "wasm32"))]
mod kithara_play {
    #[path = "../kithara_play/engine_cpal_tests.rs"]
    mod engine_cpal_tests;
}
