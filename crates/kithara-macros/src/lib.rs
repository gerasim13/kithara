//! `kithara-macros` — proc-macros shared by Kithara's production crates.
//!
//! `lib.rs` holds only the `#[proc_macro_derive]` entry point Rust requires in
//! a crate root and delegates to [`patch`].

mod patch;

use proc_macro::TokenStream;

/// `#[derive(Patch)]` — generate `<Struct>Patch`, the shape a configuration
/// document may say about a configuration struct, and the `apply` that merges
/// one onto the other.
#[proc_macro_derive(Patch, attributes(patch))]
pub fn patch(input: TokenStream) -> TokenStream {
    patch::expand(input)
}
