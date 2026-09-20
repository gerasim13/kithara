//! Byte encodings for this crate's stored artifacts.
//!
//! The versioned framing itself belongs to `kithara-signal`, beside the
//! frame-range types these blobs carry; this module holds only the `Blob`
//! implementations for analysis artifacts.

mod progress;
mod track;

#[cfg(test)]
pub(crate) use kithara_signal::to_bytes;
pub(crate) use kithara_signal::{
    Blob, BlobError, MAX_PREALLOC, Reader, Writer, from_bytes, write_to,
};
