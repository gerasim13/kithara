mod contract;
#[cfg(not(target_arch = "wasm32"))]
#[path = "native.rs"]
mod target;
#[cfg(target_arch = "wasm32")]
#[path = "wasm.rs"]
mod target;

pub use contract::{Player, PlayerControlSource};
pub use target::PlayerMember;
pub(crate) use target::PlayerSync;
