//! The application the gallery is: the readings its pages draw and the
//! endpoints they are drawn from.
//!
//! This is a demo model, not test scaffolding. It stands here because the
//! gallery is the program that shows it; what asks questions of it lives in
//! `tests/gallery/checks`, beside every other check on the gallery.

pub(crate) mod consts;
pub(crate) mod data;
mod endpoints;
mod pages;
pub(crate) mod quality;
pub(crate) mod reads;

pub(crate) use endpoints::{DemoRegistry, registry};
pub(crate) use reads::DemoReads;
