/// Stretch backend selection. Variants exist only when their backend is
/// compiled in. Selecting an absent backend is un-representable rather than a
/// runtime error.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, derive_more::Display, PartialEq, Eq)]
#[display("{self:?}")]
pub enum StretchKind {
    /// `signalsmith-stretch` (C++). Feature `stretch-signalsmith`.
    #[cfg(all(feature = "stretch-signalsmith", not(target_arch = "wasm32")))]
    Signalsmith,
    /// `bungee` (C++). Feature `stretch-bungee`.
    #[cfg(all(
        feature = "stretch-bungee",
        not(target_arch = "wasm32"),
        not(all(target_os = "windows", target_env = "msvc"))
    ))]
    Bungee,
}

impl StretchKind {
    /// Backends compiled into this target/feature set, in selector order.
    /// The DJ UI renders exactly these, so an unavailable backend is never
    /// shown nor clickable. Non-empty by construction: the crate requires at
    /// least one backend feature (`compile_error!` in `lib.rs` otherwise).
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            #[cfg(all(feature = "stretch-signalsmith", not(target_arch = "wasm32")))]
            Self::Signalsmith,
            #[cfg(all(
                feature = "stretch-bungee",
                not(target_arch = "wasm32"),
                not(all(target_os = "windows", target_env = "msvc"))
            ))]
            Self::Bungee,
        ]
    }
}

/// Stable discriminant for storing the selection in an atomic. Values are
/// fixed regardless of which feature-gated variants are compiled in.
impl From<StretchKind> for u8 {
    fn from(kind: StretchKind) -> Self {
        match kind {
            #[cfg(all(feature = "stretch-signalsmith", not(target_arch = "wasm32")))]
            StretchKind::Signalsmith => 1,
            #[cfg(all(
                feature = "stretch-bungee",
                not(target_arch = "wasm32"),
                not(all(target_os = "windows", target_env = "msvc"))
            ))]
            StretchKind::Bungee => 2,
        }
    }
}

/// Decode a stored backend discriminant. Any value outside the compiled-in set
/// decodes to the default (first compiled-in) backend.
impl From<u8> for StretchKind {
    fn from(value: u8) -> Self {
        match value {
            #[cfg(all(feature = "stretch-signalsmith", not(target_arch = "wasm32")))]
            1 => Self::Signalsmith,
            #[cfg(all(
                feature = "stretch-bungee",
                not(target_arch = "wasm32"),
                not(all(target_os = "windows", target_env = "msvc"))
            ))]
            2 => Self::Bungee,
            _ => Self::all()[0],
        }
    }
}

/// The first compiled-in backend, in [`Self::all`] selector order.
impl Default for StretchKind {
    fn default() -> Self {
        Self::all()[0]
    }
}

/// UI label = the variant name (`Signalsmith` / `Bungee`), via `Debug`, so
/// the selector needs no per-variant `cfg` arm.
#[cfg(test)]
#[path = "kind_tests.rs"]
mod tests;
