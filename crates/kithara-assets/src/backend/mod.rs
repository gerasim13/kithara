mod deletion;
#[cfg(not(target_arch = "wasm32"))]
mod disk;
mod memory;

pub(crate) use deletion::AssetDeleter;
#[cfg(not(target_arch = "wasm32"))]
pub use disk::DiskAssetStore;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use disk::{DEFAULT_SEGMENT_RESERVATION, DiskAssetDeleter};
pub use memory::MemAssetStore;
pub(crate) use memory::{MemAssetDeleter, MemStoreSetup};
