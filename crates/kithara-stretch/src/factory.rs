#[cfg(all(
    feature = "stretch-bungee",
    not(target_arch = "wasm32"),
    not(all(target_os = "windows", target_env = "msvc"))
))]
use crate::backends::BungeeBackend;
#[cfg(all(feature = "stretch-signalsmith", not(target_arch = "wasm32")))]
use crate::backends::SignalsmithBackend;
use crate::{StretchBackend, StretchKind, StretchOptions};

/// Construct the backend for `kind` at the configured source shape. Called once
/// per chain build and on a source-spec change inside the audio processor.
#[must_use]
pub fn build_backend(kind: StretchKind, options: &StretchOptions) -> Box<dyn StretchBackend> {
    match kind {
        #[cfg(all(feature = "stretch-signalsmith", not(target_arch = "wasm32")))]
        StretchKind::Signalsmith => Box::new(SignalsmithBackend::new(options)),
        #[cfg(all(
            feature = "stretch-bungee",
            not(target_arch = "wasm32"),
            not(all(target_os = "windows", target_env = "msvc"))
        ))]
        StretchKind::Bungee => Box::new(BungeeBackend::new(options)),
    }
}

#[cfg(test)]
#[path = "factory_tests.rs"]
mod tests;
