//! Shared infrastructure for xtask static-analysis namespaces (`arch`, `style`, ...).
//!
//! Provides:
//! - `Violation` / `Severity` / `Report` — uniform check results
//! - `Baseline` / `RatchetDiff` — ratchet baseline plumbing
//! - `walker` — `.rs` discovery and glob matching
//! - `scan` — one shared walk and read of the workspace per ratchet run
//! - `parse` — `syn` AST helpers (file parsing, scope/impl traversal, passthrough analysis)
//! - `report` — markdown / JSON renderers

pub mod baseline;
pub mod exclude;
pub mod fix;
mod libtest;
pub mod parse;
pub(crate) mod process;
pub mod project;
pub mod report;
pub mod scan;
pub mod scope;
pub mod style;
pub mod suppress;
pub mod timestamp;
pub mod tools;
pub mod violation;
pub mod walker;

#[cfg(test)]
pub(crate) use libtest::child_test_args;
