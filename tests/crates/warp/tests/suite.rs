#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]

use kithara_test_utils::bufpool as test_pools;

#[cfg(feature = "playback")]
mod real_track;
mod region;
