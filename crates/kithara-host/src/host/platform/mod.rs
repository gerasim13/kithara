#[cfg(not(target_arch = "wasm32"))]
mod native;
mod result;
#[cfg(target_arch = "wasm32")]
mod wasm;

#[cfg(not(target_arch = "wasm32"))]
pub(super) use native::Platform;
pub(super) use result::PlatformResult;
#[cfg(target_arch = "wasm32")]
pub(super) use wasm::Platform;
