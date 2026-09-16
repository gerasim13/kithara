//! `kithara-derive` — the derive macros shared by Kithara's production crates.
//!
//! `lib.rs` holds only the `#[proc_macro_derive]` entry points Rust requires in
//! a crate root and delegates to the module that owns each expansion.

mod config;
#[cfg(feature = "event")]
mod event;
mod phase;
mod ranged;
mod ui;

use proc_macro::TokenStream;

/// Implements `Default` by calling the type's existing no-input builder.
#[cfg(feature = "built-default")]
#[proc_macro_derive(BuiltDefault)]
pub fn built_default(input: TokenStream) -> TokenStream {
    config::built::expand(input)
}
#[cfg(feature = "event")]
use syn::{DeriveInput, Error, parse_macro_input};

/// `#[derive(Patch)]` — generate `<Struct>Patch`, the shape a configuration
/// document may say about a configuration struct, and the `apply` that merges
/// one onto the other.
#[cfg(feature = "patch")]
#[proc_macro_derive(Patch, attributes(patch))]
pub fn patch(input: TokenStream) -> TokenStream {
    config::expand(input)
}

/// Implements the immediate UI host path shared by draw-only controls.
#[cfg(feature = "ui")]
#[proc_macro_derive(ViewControl)]
pub fn view_control(input: TokenStream) -> TokenStream {
    ui::view::expand(input)
}

/// Implements one of Kithara's closed typestate phase traits.
#[cfg(feature = "phase")]
#[proc_macro_derive(Phase, attributes(phase))]
pub fn phase(input: TokenStream) -> TokenStream {
    phase::expand(input)
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
#[cfg(feature = "ranged")]
#[proc_macro_derive(Ranged, attributes(ranged))]
pub fn ranged(input: TokenStream) -> TokenStream {
    ranged::expand(input)
}

/// Implements the marker trait for a concrete event struct or enum.
#[cfg(feature = "event")]
#[proc_macro_derive(Event)]
pub fn event(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    event::derive(&input)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

/// Implements a consumer set of concrete event types.
#[cfg(feature = "event")]
#[proc_macro_derive(EventSet)]
pub fn event_set(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    event::derive_set(&input)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}
