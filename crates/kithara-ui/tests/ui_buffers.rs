#![cfg(all(feature = "capture", feature = "masonry"))]

//! What the gallery asks of the renderer's buffers, alone in its own binary.
//!
//! It draws, so it cannot share a binary with the memory budget beside it: the
//! graphics device counts bytes for the whole process, and two drawing tests in
//! one process read each other's allocations.
#[path = "../examples/gallery/app.rs"]
mod app;
#[path = "../examples/gallery/capture.rs"]
mod capture;
#[path = "../examples/gallery/cli.rs"]
mod cli;
#[path = "../examples/gallery/custom.rs"]
mod custom;
#[path = "../examples/gallery/demo/mod.rs"]
mod demo;
#[path = "../examples/gallery/fixture.rs"]
mod fixture;
#[path = "../examples/gallery/host.rs"]
mod host;
#[path = "../examples/gallery/sections.rs"]
mod sections;

// A sibling in `tests/` would be a test binary of its own, and this one carries
// the gallery modules the checks are written against.
#[path = "ui_buffers/checks.rs"]
mod checks;
