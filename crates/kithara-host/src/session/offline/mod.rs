pub(crate) mod backend;
mod client;
#[cfg(not(target_arch = "wasm32"))]
mod native;
mod task;
#[cfg(target_arch = "wasm32")]
mod wasm;

pub(crate) use client::OfflineSessionClient;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::{OfflineTaskHandle, OfflineTaskRoute};
pub(crate) use task::{OfflineSessionError, OfflineTaskConfig, spawn};
#[cfg(target_arch = "wasm32")]
pub(crate) use wasm::{OfflineTaskHandle, OfflineTaskRoute};
