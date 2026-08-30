#![forbid(unsafe_code)]

//! File streaming implementation for progressive HTTP downloads.

mod config;
mod coord;
mod error;
mod session;
mod stream;
#[cfg(test)]
mod test_pools;

pub use config::{FileConfig, FileSrc};
pub use stream::File;
