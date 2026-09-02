#![forbid(unsafe_code)]

//! Storage-neutral recording over Kithara's continuous encoder sessions.

mod config;
mod core;
mod error;
mod sink;

pub use core::RecordingCore;

pub use config::RecordingConfig;
pub use error::{RecordingError, RecordingResult};
pub use sink::RecordingSink;
