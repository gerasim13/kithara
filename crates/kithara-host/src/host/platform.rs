#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod wasm;

#[cfg(not(target_arch = "wasm32"))]
pub(super) use native::Platform;
#[cfg(target_arch = "wasm32")]
pub(super) use wasm::Platform;
