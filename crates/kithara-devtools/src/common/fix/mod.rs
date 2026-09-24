//! Comment-preserving autofix engine for xtask checks.
//!
//! See `README.md` next to this module for the full contract: how block
//! ranges are computed, what counts as leading vs trailing trivia, and
//! the four invariants (I1-I4) that every check fix must satisfy.
//!
//! This module deliberately stays low-level: it does not know about specific
//! checks or about `Violation`. Each style/arch check that opts in will use
//! `BlockExtractor` to derive byte ranges and `SourceRewriter` to apply
//! non-overlapping edits.

pub(crate) mod block;
mod outcome;
pub(crate) mod rewriter;
#[cfg(test)]
mod tests;

pub use block::{
    BlockRange, ExpansionError, deletion_range, expand_blocks, leading_trivia_start, line_start,
};
pub use outcome::FixOutcome;
pub use rewriter::SourceRewriter;
