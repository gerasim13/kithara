#![forbid(unsafe_code)]

//! File streaming implementation for progressive HTTP downloads.
//!
//! # Example
//!
//! ```ignore
//! use kithara_assets::AssetStore;
//! use kithara_file::{File, FileConfig, FileSrc};
//! use kithara_stream::{Stream, StreamType};
//!
//! // Using StreamType API
//! let store = AssetStore::builder().build();
//! let config = FileConfig::for_src(FileSrc::Remote(url))
//!     .store(store)
//!     .build();
//! let inner = File::create(config).await?;
//! ```

mod config;
mod coord;
mod error;
mod session;
mod stream;

pub use config::{FileConfig, FileSrc};
pub use stream::File;
