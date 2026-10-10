//! Portable repeated-test runs and independent evidence verification.

mod artifacts;
mod command;
#[cfg(test)]
mod coverage_tests;
mod environment;
mod manifest;
mod output;
pub(crate) mod pressure;
mod selection;
mod system;

pub use command::{ReportArgs, RunArgs, StressCommand};
pub(crate) use command::{run, run_output, run_stderr_output};
