mod audio;
mod source_span;

#[cfg(test)]
mod source_span_tests;

pub use audio::{AudioChunk, AudioChunkInfo};
pub use source_span::SourceSpan;
