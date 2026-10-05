#![forbid(unsafe_code)]

#[cfg(not(target_arch = "wasm32"))]
use kithara_test_dylib as _;

mod common;

#[cfg(not(target_arch = "wasm32"))]
mod multi_instance;

mod offline_browser;
