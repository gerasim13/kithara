#![forbid(unsafe_code)]

//! Build-time generated audio test assets.
//!
//! Generators live in `src/defs/`, compile only into this crate's build script,
//! and never enter the library. See `CONTEXT.md` for the store layout and the
//! invalidation contract.

pub mod asset;
pub mod assets;
// Read by this crate's build script through `#[path]`, and still by the
// integration suite's; declared here so its own tests keep running.
#[cfg(test)]
mod encoder_crates;
#[cfg(not(target_arch = "wasm32"))]
pub mod store;
