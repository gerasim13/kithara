use kithara::{
    platform::CancelToken,
    storage::{MemOptions, MemResource},
};

/// Build a committed in-memory resource pre-filled with `data`.
///
/// Mirrors the old `MemResource::with_bytes` test constructor over the
/// public `MemResource::open` API.
#[must_use]
pub fn mem_resource_with_bytes(data: &[u8], cancel: CancelToken) -> MemResource {
    MemResource::open(
        cancel,
        MemOptions::builder().initial_data(data.to_vec()).build(),
    )
    .expect("BUG: MemDriver::open with initial_data is infallible")
}
