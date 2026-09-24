#[cfg(target_arch = "wasm32")]
pub use futures::lock::{Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard};
#[cfg(not(target_arch = "wasm32"))]
pub use tokio::sync::{Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard};
