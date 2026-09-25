//! Documents, their compilation and the built-in presets, driven through the
//! public API.

mod builtin_presets;
#[path = "../common/mod.rs"]
mod common;
mod compile;
mod document_group;
mod document_object;
mod document_placed;
mod document_wave;
mod multi_deck;
mod roundtrip;
mod skin;
mod swatch;
mod text;
