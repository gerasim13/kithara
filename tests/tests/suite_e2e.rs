#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    reason = "integration test crate — unwraps are acceptable in test code"
)]
//! What puts a test here is the machine, not the code. These tests open a real
//! cpal output stream, and a container has no sound card. The half that needs no
//! device now lives in `suite_light` and runs in the ordinary gate.
//!
//! The Apple executor serves this suite through `apple:e2e`, because that
//! machine has hardware. A developer machine can serve it with
//! `just test run --lane=e2e`. No container lane carries it: a runner that
//! failed here would be reporting the fence around itself rather than the code.

pub use kithara_integration_tests::bufpool_ext;

#[cfg(not(target_arch = "wasm32"))]
mod kithara_play {
    #[path = "../kithara_play/engine_cpal_tests.rs"]
    mod engine_cpal_tests;
    #[path = "../kithara_play/engine_session_contract.rs"]
    mod engine_session_contract;
}
