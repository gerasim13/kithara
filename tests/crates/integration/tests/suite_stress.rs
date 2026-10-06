#![forbid(unsafe_code)]
#![recursion_limit = "256"]

#[cfg(not(target_arch = "wasm32"))]
use kithara_test_dylib as _;

mod hls_seek_middle_stress_long;
