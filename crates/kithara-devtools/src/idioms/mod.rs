//! Idiomatic-construction fitness functions for the workspace.
//!
//! Run via `just lint idioms`. Same shape as `arch` and `style`: declarative
//! rules from `.config/idioms/*.toml`, ratchet baseline at
//! `.config/idioms/baseline.toml`. The namespace flags constructions that
//! compile and pass clippy but suggest a better Rust pattern (performance,
//! readability, expressivity).

mod checks;
mod command;
mod config;

pub use command::IdiomsArgs;
pub(crate) use command::run;
