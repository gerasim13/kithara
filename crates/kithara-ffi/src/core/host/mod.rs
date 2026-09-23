mod config;
pub(crate) mod lifecycle;

#[cfg(not(target_arch = "wasm32"))]
pub use config::ensure_default_host;
pub use config::{FfiHostConfig, default_host_config, initialize_host};
