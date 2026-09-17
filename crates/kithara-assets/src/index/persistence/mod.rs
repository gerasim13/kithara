#[cfg(not(target_arch = "wasm32"))]
mod file;
mod flush;
pub mod schema;
#[cfg(not(target_arch = "wasm32"))]
mod worker;
#[cfg(target_arch = "wasm32")]
#[path = "worker_stub.rs"]
mod worker;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use file::IndexFile;
pub use flush::{FlushHub, FlushPolicy, FlushPolicyPatch};
pub(crate) use flush::{Flushable, flush_sync};
