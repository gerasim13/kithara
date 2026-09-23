mod core;
#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod wasm;

pub(super) use core::PlatformResult;

#[cfg(not(target_arch = "wasm32"))]
pub(super) use native::Platform;
#[cfg(target_arch = "wasm32")]
pub(super) use wasm::Platform;
