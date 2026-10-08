#![forbid(unsafe_code)]

mod core;
mod outcome;
mod read;
mod seek;
mod variant;

pub use core::{Stream, StreamType};

pub use outcome::{StreamPending, StreamReadError, StreamReadOutcome};
pub use seek::{StreamSeekPastEof, resolve_seek_target};
pub use variant::{VariantChangeError, format_change_segment_range};
