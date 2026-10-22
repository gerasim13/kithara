#![forbid(unsafe_code)]

//! Mmap-backed storage driver.
//!
//! The struct + factory live in `driver`, the I/O ops live in `io`.

pub(crate) mod driver;
pub(crate) mod io;

pub use driver::{MmapDriver, MmapOptions, MmapResource};
