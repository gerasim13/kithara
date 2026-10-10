mod consts;
mod core;

#[cfg(all(feature = "all", not(target_arch = "wasm32")))]
mod ramped;

#[cfg(all(feature = "all", not(target_arch = "wasm32")))]
pub use core::open_resource;
pub use core::{LaneAudio, PcmDeck, load_audio, load_source_audio, wait_for_preload};

pub use consts::{PRELOAD_READY_RETRIES, READ_PENDING_POLL};
#[cfg(all(feature = "all", not(target_arch = "wasm32")))]
pub use ramped::{InjectedFactory, RampedFactory};

#[cfg(all(test, feature = "all", not(target_arch = "wasm32")))]
mod tests;
