//! Explicit native Tokio surface; time, files and root task spawns stay outside it.
//! Sockets, child processes and shutdown signals are opt-in.

#[cfg(feature = "signal")]
pub use backend::signal;
pub use backend::{join, main, pin, select, task_local, try_join};
pub(crate) use tokio as backend;

#[cfg(feature = "tokio-net")]
pub mod io {
    pub use super::backend::io::{AsyncReadExt, AsyncWriteExt};
}

#[cfg(feature = "tokio-net")]
pub mod net {
    pub use super::backend::net::{TcpListener, TcpStream};
}
#[cfg(all(feature = "tokio-process", not(target_arch = "wasm32")))]
pub mod process {
    pub use super::backend::process::Command;
}
pub mod runtime;
pub mod sync;
pub mod task;
pub(crate) mod thread_pool;

pub use thread_pool::ensure_thread_pool;
