mod core;
#[cfg(not(target_arch = "wasm32"))]
#[path = "native.rs"]
mod platform;
#[cfg(target_arch = "wasm32")]
#[path = "wasm.rs"]
mod platform;

pub(crate) use core::{Budget, ComputeRuntime};
pub use core::{ComputeContext, ComputeRejected, ComputeSubmitError};

pub(crate) use platform::ComputePool;
