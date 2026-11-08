#![cfg(not(target_arch = "wasm32"))]

mod cases;
mod gain;
#[cfg(not(target_os = "android"))]
mod numerical;
mod starvation;

pub(super) use cases::{
    ROOMY_CAPACITY, play_resource, play_source, playing_harness, render_blocks,
};
