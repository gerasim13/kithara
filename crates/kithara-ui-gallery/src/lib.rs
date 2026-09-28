//! The gallery: every page the toolkit's documents draw, the demo host that
//! answers what those pages read, and the harnesses that photograph and
//! compare them. The `gallery` binary opens them in a window; the suites
//! beside it check these same modules.

pub mod app;
pub mod capture;
pub mod cli;
pub mod custom;
pub mod demo;
pub mod fixture;
#[cfg(feature = "masonry")]
pub mod host;
pub mod sections;
