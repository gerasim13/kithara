pub use config::{DrawPoolLimits, Limits, UiConfig};
#[cfg(not(target_arch = "wasm32"))]
pub use file::FileResolver;
pub use mem::MemResolver;
pub use uri::{LoadedBytes, LoadedSource, SourceResolver};
pub(crate) use uri::{base_dir, join_rel, resolve_uri};

mod config;
#[cfg(not(target_arch = "wasm32"))]
mod file;
mod mem;
mod uri;
