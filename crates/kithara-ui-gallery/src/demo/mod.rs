//! The application the gallery is: the readings its pages draw and the
//! endpoints they are drawn from.
//!
//! This is a demo model, not test scaffolding. It stands here because the
//! gallery is the program that shows it; what asks questions of it lives in
//! `tests/gallery/checks`, beside every other check on the gallery.

pub mod consts;
pub mod data;
mod endpoints;
mod pages;
pub mod quality;
pub mod reads;

pub use endpoints::{DemoRegistry, registry};
pub use reads::DemoReads;
