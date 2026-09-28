//! The native backend this build selects: the standard library, loom, or the
//! flash scheduler over either. Each variant exposes the same module set.

#[cfg(feature = "flash")]
mod flash;
#[cfg(all(feature = "flash", feature = "loom"))]
mod flash_loom;
#[cfg(all(feature = "flash", not(feature = "loom")))]
mod flash_system;
#[cfg(all(feature = "loom", not(feature = "flash")))]
mod loom;

#[cfg(all(feature = "flash", feature = "loom"))]
pub use flash_loom::*;
#[cfg(all(feature = "flash", not(feature = "loom")))]
pub use flash_system::*;
#[cfg(all(feature = "loom", not(feature = "flash")))]
pub use loom::*;

#[cfg(not(any(feature = "flash", feature = "loom")))]
pub use crate::system::{logging, maybe_send, model::model, sync, thread, time, tokio};
