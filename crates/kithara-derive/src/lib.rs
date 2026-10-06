//! `kithara-derive` — the derive macros shared by Kithara's production crates.
//!
//! `lib.rs` holds only the `#[proc_macro_derive]` entry points Rust requires in
//! a crate root and delegates to the module that owns each expansion.

mod config;
#[macro_use]
mod entrypoints;
mod event;
mod mirror;
mod phase;
mod ranged;
mod ui;
mod vocabulary;

#[cfg(feature = "config")]
config::retained::config_derives!();

config::config_derives!();

ui_derives!();

/// Implements ordered traversal of frame and text-role fields in a skin structure.
#[cfg(feature = "skin-walk")]
#[proc_macro_derive(SkinWalk, attributes(skin))]
pub fn skin_walk(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    ui::skin::expand(input)
}

/// Implements one of Kithara's closed typestate phase traits.
#[cfg(feature = "phase")]
#[proc_macro_derive(Phase, attributes(phase))]
pub fn phase(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    phase::expand(input)
}

/// Declares a bounded numeric newtype.
///
/// `checked` and `Deserialize` refuse out-of-range values; the optional
/// `clamp` flag adds a clamping `From` and requires a declared default.
#[cfg(feature = "ranged")]
#[proc_macro_derive(Ranged, attributes(ranged))]
pub fn ranged(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    ranged::expand(input)
}

#[cfg(all(doctest, feature = "ranged"))]
mod ranged_contracts {
    /// Unchecked conversion requires an explicitly clamping declaration.
    /// ```compile_fail
    /// #[derive(kithara_derive::Ranged)]
    /// #[ranged(min = 0, max = 100)]
    /// struct Share(u8);
    /// let share = Share::from(101u8);
    /// ```
    mod rejects_implicit_from {}

    /// Default construction requires a declared default value.
    /// ```compile_fail
    /// #[derive(kithara_derive::Ranged)]
    /// #[ranged(min = 1.0, max = 1000.0)]
    /// struct Tempo(f64);
    /// let tempo = Tempo::default();
    /// ```
    mod rejects_undeclared_default {}
}

/// Implements a complete structural conversion between two product models.
#[cfg(feature = "mirror")]
#[proc_macro_derive(Mirror, attributes(mirror))]
pub fn mirror(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    mirror::expand(input)
}

#[cfg(any(feature = "enum-str", feature = "variants"))]
vocabulary_derives!();

#[cfg(feature = "event")]
event_derives!();
