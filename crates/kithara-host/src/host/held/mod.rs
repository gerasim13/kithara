#[cfg(not(target_arch = "wasm32"))]
#[path = "native.rs"]
mod target;
#[cfg(target_arch = "wasm32")]
#[path = "wasm.rs"]
mod target;

pub(crate) use target::HeldPlayer;
