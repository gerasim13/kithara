#![forbid(unsafe_code)]

//! Integration tests for kithara-storage

#[cfg(target_os = "android")]
use kithara_test_dylib as _;

mod atomic;
#[cfg(not(target_arch = "wasm32"))]
mod availability;
mod streaming;
mod support;
