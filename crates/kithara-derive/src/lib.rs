//! `kithara-derive` — the derive macros shared by Kithara's production crates.
//!
//! `lib.rs` holds only the `#[proc_macro_derive]` entry points Rust requires in
//! a crate root and delegates to the module that owns each expansion.

mod patch;
mod ranged;

use proc_macro::TokenStream;

/// `#[derive(Patch)]` — generate `<Struct>Patch`, the shape a configuration
/// document may say about a configuration struct, and the `apply` that merges
/// one onto the other.
///
/// See the crate `README.md` for the field attributes and `CONTEXT.md` for the
/// contract the generated code keeps.
#[proc_macro_derive(Patch, attributes(patch))]
pub fn patch(input: TokenStream) -> TokenStream {
    patch::expand(input)
}

/// Declares a bounded numeric newtype.
///
/// `checked` and `Deserialize` refuse out-of-range values; the optional
/// `clamp` flag adds a clamping `From` and requires a declared default.
#[proc_macro_derive(Ranged, attributes(ranged))]
pub fn ranged(input: TokenStream) -> TokenStream {
    ranged::expand(input)
}
