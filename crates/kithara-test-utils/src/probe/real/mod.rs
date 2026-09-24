#![cfg_attr(target_arch = "wasm32", allow(unused_imports))]

/// `DTrace` is the native USDT backend on macOS. Other targets use the tracing
/// backend emitted by the probe macro.
#[cfg(all(target_os = "macos", not(miri)))]
mod usdt_wire;
mod wire;

#[cfg(all(target_os = "macos", not(miri)))]
pub use usdt_wire::{fire_0, fire_1, fire_2, fire_3, fire_4, fire_5};
pub use wire::register_probes;

#[cfg(all(test, target_os = "macos", not(miri)))]
mod tests;
