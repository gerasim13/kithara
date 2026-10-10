mod core;
#[cfg(not(any(feature = "keystore", target_arch = "wasm32")))]
mod file;

pub use self::core::{SecretError, Secrets};
