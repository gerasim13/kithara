//! Pure native backend. Knows nothing about the other backends;
//! cross-platform code lives in `crate::common` and is re-imported here.
//! Compiled only off wasm32 (gated in `lib.rs`). Every native build takes
//! its logging, `WasmSend` bound, and ownership types from here; the flash
//! backends replace the thread, time, and Tokio modules, and loom replaces
//! the lock primitives.

#[cfg(not(any(feature = "flash", feature = "loom")))]
pub(crate) mod errors;
#[cfg(feature = "flash")]
pub(crate) mod lock;
pub mod logging;
pub mod maybe_send;
#[cfg(not(any(feature = "flash", feature = "loom")))]
pub(crate) mod model;
pub(crate) mod ownership;
#[cfg(feature = "loom")]
pub(crate) mod poison;
#[cfg(not(any(feature = "flash", feature = "loom")))]
pub mod sync;
#[cfg(not(any(feature = "flash", feature = "loom")))]
pub mod thread;
#[cfg(not(feature = "flash"))]
pub mod time;
#[cfg(not(feature = "flash"))]
pub mod tokio;
