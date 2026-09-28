//! Platform-aware primitives with one compile-time-selected backend.
//! Backends mirror the public sync, thread, time, and Tokio facade.

mod common;

#[cfg(not(target_arch = "wasm32"))]
mod backend;
#[cfg(all(not(target_arch = "wasm32"), feature = "loom"))]
mod loom;
#[cfg(not(target_arch = "wasm32"))]
mod system;
#[cfg(all(not(target_arch = "wasm32"), not(feature = "flash")))]
pub use backend::{logging, maybe_send, sync, thread, time, tokio};

#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub mod __private;

#[cfg(target_arch = "wasm32")]
mod wasm;
#[cfg(target_arch = "wasm32")]
pub use wasm::*;

#[cfg(all(not(target_arch = "wasm32"), feature = "flash"))]
pub mod flash;
#[cfg(not(all(not(target_arch = "wasm32"), feature = "flash")))]
pub use common::flash_inert as flash;
#[cfg(all(not(target_arch = "wasm32"), feature = "no-block"))]
pub mod no_block;
#[cfg(not(all(not(target_arch = "wasm32"), feature = "no-block")))]
pub use common::no_block_inert as no_block;
pub use common::{
    async_lock::{AsyncMutex, AsyncMutexGuard},
    cancel::{CancelGroup, CancelScope, CancelToken, CancelWakerGuard, Cancelled},
    traits,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "flash"))]
pub use flash::*;
mod consts;
