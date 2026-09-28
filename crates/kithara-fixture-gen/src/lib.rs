#![forbid(unsafe_code)]

//! Build-time fixture generation. Every `#[kithara::asset]` definition in
//! `defs` registers itself at link time; [`generate`] orders them by what they
//! are built from, materializes each into the shared store, and writes the
//! accessors the fixture crate compiles. Nothing here reaches a target build.

mod context;
mod defs;
mod generate;
mod graph;
#[cfg(feature = "remote")]
mod hls_hydrate;
mod registry;
#[cfg(feature = "library")]
mod remote_file;

pub use generate::generate;
#[cfg(feature = "packaged")]
use kithara_fixture_media::fmp4;
#[cfg(feature = "hls")]
use kithara_fixture_media::hls_manifest;
#[cfg(feature = "hls-inputs")]
use kithara_fixture_media::variant_input;
use kithara_fixture_media::{signal, store};
