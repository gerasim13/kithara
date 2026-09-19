#![cfg(all(feature = "hls", not(target_arch = "wasm32")))]

use std::fs;

use kithara_test_macros as kithara;

use crate::hls;

/// Prepared init or media bytes from the build-time HLS bundle.
#[kithara::fixture]
#[must_use]
/// # Panics
/// Panics if the build-time HLS manifest omits the required resource.
pub fn aac_init() -> Vec<u8> {
    resource("/hls/init-slq-a1.mp4")
}

/// Prepared init or media bytes from the build-time HLS bundle.
#[kithara::fixture]
#[must_use]
/// # Panics
/// Panics if the build-time HLS manifest omits the required resource.
pub fn aac_segment() -> Vec<u8> {
    resource("/hls/segment-1-slq-a1.m4s")
}

/// Prepared init or media bytes from the build-time HLS bundle.
#[kithara::fixture]
#[must_use]
/// # Panics
/// Panics if the build-time HLS manifest omits the required resource.
pub fn flac_init() -> Vec<u8> {
    resource("/hls/init-slossless-a1.mp4")
}

fn resource(route: &str) -> Vec<u8> {
    let resource = hls::long_plain().get(route).expect("prepared HLS resource");
    fs::read(resource.path()).expect("read prepared HLS bytes")
}
