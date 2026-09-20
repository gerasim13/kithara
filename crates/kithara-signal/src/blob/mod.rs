//! Versioned little-endian byte framing for stored analysis artifacts.
//!
//! The framing lives beside the frame-range types it encodes so an artifact
//! owner can serialize itself without depending on an analyzer.

mod frame;
mod read;
mod write;

pub use frame::{Blob, BlobError, MAX_PREALLOC, from_bytes, to_bytes, write_to};
pub use read::Reader;
pub use write::Writer;
