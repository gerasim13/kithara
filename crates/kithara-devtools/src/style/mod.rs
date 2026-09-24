//! Code-style fitness functions for the workspace.
//!
//! Run via `just lint style`. Reads declarative rules from `.config/style/*.toml`,
//! evaluates them against the workspace, and ratchets results against
//! `.config/style/baseline.toml`. Same shape as `arch`, but with a separate
//! baseline and config tree to keep topological and stylistic concerns split.

mod checks;
mod command;
mod config;

pub use command::StyleArgs;
pub(crate) use command::run;
