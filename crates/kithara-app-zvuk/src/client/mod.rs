mod core;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

pub use core::Client;
