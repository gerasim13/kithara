//! Flash-aware Tokio channels and tasks with explicit native runtime plumbing.
//! Time, files and root task spawns stay outside this surface.
//! Native sockets, child processes and shutdown signals are opt-in.

#[cfg(all(feature = "tokio-process", not(target_arch = "wasm32")))]
pub use crate::backend::tokio::process;
#[cfg(feature = "signal")]
pub use crate::backend::tokio::signal;
#[cfg(feature = "tokio-net")]
pub use crate::backend::tokio::{io, net};
pub use crate::backend::tokio::{join, main, pin, select, task_local, try_join};

pub mod broadcast;
mod errors;
pub mod mpsc;
pub mod oncecell;
pub mod oneshot;
pub mod semaphore;
pub mod sync;
pub mod task;
mod unbounded;
pub mod watch;

pub use crate::backend::tokio::{runtime, thread_pool::ensure_thread_pool};
