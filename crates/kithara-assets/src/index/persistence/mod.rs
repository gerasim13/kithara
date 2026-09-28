#[cfg(not(target_arch = "wasm32"))]
mod file;
mod flush;
pub mod schema;
#[cfg(not(target_arch = "wasm32"))]
mod worker;
#[cfg(target_arch = "wasm32")]
mod worker_stub;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use file::IndexFile;
pub use flush::{FlushHub, FlushPolicy, FlushPolicyPatch};
pub(crate) use flush::{Flushable, flush_sync};
#[cfg(target_arch = "wasm32")]
use worker_stub as worker;
