//! Architectural fitness functions for the workspace.
//!
//! Run via `just lint arch`. Reads declarative rules from `.config/arch/*.toml`,
//! evaluates them against the workspace, and ratchets results against
//! `.config/arch/baseline.toml`.

mod checks;
mod command;
mod config;

pub use command::ArchArgs;
pub(crate) use command::{redundant_accessor_keys, run};
