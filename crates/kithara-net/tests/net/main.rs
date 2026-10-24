#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    reason = "integration test crate - unwraps are acceptable in test code"
)]

//! Integration tests for kithara-net

#[cfg(target_os = "android")]
use kithara_test_dylib as _;

mod http_client;
#[cfg(all(feature = "flash", not(target_arch = "wasm32")))]
mod supplied_transport;
