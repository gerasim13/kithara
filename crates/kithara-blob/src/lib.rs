//! Versioned little-endian byte framing for stored artifacts.
//!
//! An artifact owner implements [`Blob`] to write and read its own body; this
//! crate frames the version header and hands the body a cursor that refuses to
//! trust a length prefix. It knows nothing about what it carries.

mod frame;
mod read;
mod write;

pub use frame::{Blob, BlobError, MAX_PREALLOC, from_bytes, to_bytes, write_to};
pub use read::Reader;
pub use write::Writer;
