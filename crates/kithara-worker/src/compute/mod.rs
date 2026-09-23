#[cfg(not(target_arch = "wasm32"))]
#[path = "native.rs"]
mod platform;
#[cfg(target_arch = "wasm32")]
#[path = "wasm.rs"]
mod platform;
mod runtime;

pub(crate) use platform::ComputePool;
pub(crate) use runtime::{Budget, ComputeRuntime};
pub use runtime::{ComputeContext, ComputeRejected, ComputeSubmitError};
