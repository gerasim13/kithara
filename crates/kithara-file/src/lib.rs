#![forbid(unsafe_code)]

//! File streaming implementation for progressive HTTP downloads.
//!
//! # Example
//!
//! ```ignore
//! use kithara_assets::AssetStoreBuilder;
//! use kithara_file::{File, FileConfig, FileSrc};
//! use kithara_stream::{Stream, StreamType};
//!
//! // Using StreamType API
//! let store = AssetStoreBuilder::default().build();
//! let config = FileConfig::new(FileSrc::Remote(url), store);
//! let inner = File::create(config).await?;
//! ```

mod config;
mod coord;
pub(crate) mod engine;
mod error;
mod session;
mod stream;

pub use config::{FetchCompleteFn, FetchOutcome, FileConfig, FileSrc, SlowFn};
pub use stream::File;
