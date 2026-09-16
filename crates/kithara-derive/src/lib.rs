//! `kithara-derive` — the derive macros shared by Kithara's production crates.
//!
//! `lib.rs` holds only the `#[proc_macro_derive]` entry points Rust requires in
//! a crate root and delegates to the module that owns each expansion.

mod event;
mod event_set;
mod patch;
mod ranged;

use proc_macro::TokenStream;
use syn::{DeriveInput, Error, parse_macro_input};

/// `#[derive(Patch)]` — generate `<Struct>Patch`, the shape a configuration
/// document may say about a configuration struct, and the `apply` that merges
/// one onto the other.
#[proc_macro_derive(Patch, attributes(patch))]
pub fn patch(input: TokenStream) -> TokenStream {
    patch::expand(input)
}

/// Declares a bounded numeric newtype.
///
/// `checked` and `Deserialize` refuse out-of-range values; the optional
/// `clamp` flag adds a clamping `From` and requires a declared default.
///
/// ```compile_fail
/// #[derive(kithara_derive::Ranged)]
/// #[ranged(min = 0, max = 100)]
/// struct Share(u8);
/// let share = Share::from(101u8);
/// ```
///
/// ```compile_fail
/// #[derive(kithara_derive::Ranged)]
/// #[ranged(min = 1.0, max = 1000.0)]
/// struct Tempo(f64);
/// let tempo = Tempo::default();
/// ```
#[proc_macro_derive(Ranged, attributes(ranged))]
pub fn ranged(input: TokenStream) -> TokenStream {
    ranged::expand(input)
}

/// Implements the marker trait for a concrete event struct or enum.
#[proc_macro_derive(Event)]
pub fn event(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    event::derive(&input)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

/// Implements a consumer set of concrete event types.
#[proc_macro_derive(EventSet)]
pub fn event_set(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    event_set::derive(&input)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}
