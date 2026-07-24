#![forbid(unsafe_code)]

//! Per-session state and Source/Peer implementations for the file protocol.
//!
//! Split into focused submodules:
//! - [`peer`] — thin `Peer` adapter over the resource engine.
//! - [`inner`] — File lifecycle sink and open-state wiring.
//! - [`source`] — public `FileSource` API and `Source` trait implementation.

mod inner;
mod peer;
mod reader;
mod segments;
mod source;
#[cfg(test)]
mod tests;

pub(crate) use inner::{FileSourceCtx, FileStreamState};
pub(crate) use peer::FilePeer;
pub(crate) use source::FileSource;
