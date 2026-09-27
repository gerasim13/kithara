#[cfg(not(target_arch = "wasm32"))]
mod native;
mod runtime;
#[cfg(target_arch = "wasm32")]
mod wasm;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::ComputePool;
pub(crate) use runtime::{Budget, ComputeRuntime};
pub use runtime::{ComputeContext, ComputeRejected, ComputeSubmitError};
#[cfg(target_arch = "wasm32")]
pub(crate) use wasm::ComputePool;
