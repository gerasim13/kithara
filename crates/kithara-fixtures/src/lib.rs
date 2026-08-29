#![forbid(unsafe_code)]

//! Build-time generated audio test assets.
//!
//! Generators live in `src/defs/`, compile only into this crate's build script,
//! and never enter the library. See `CONTEXT.md` for the store layout and the
//! invalidation contract.

#[cfg(not(target_arch = "wasm32"))]
pub mod store;
