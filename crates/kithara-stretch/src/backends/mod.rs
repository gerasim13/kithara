#[cfg(all(
    feature = "stretch-bungee",
    not(target_arch = "wasm32"),
    not(all(target_os = "windows", target_env = "msvc"))
))]
mod bungee;
#[cfg(all(feature = "stretch-signalsmith", not(target_arch = "wasm32")))]
mod signalsmith;

#[cfg(all(
    feature = "stretch-bungee",
    not(target_arch = "wasm32"),
    not(all(target_os = "windows", target_env = "msvc"))
))]
pub(crate) use bungee::BungeeBackend;
#[cfg(all(
    feature = "stretch-bungee",
    not(target_arch = "wasm32"),
    not(all(target_os = "windows", target_env = "msvc"))
))]
pub use bungee::BungeeElastic;
#[cfg(all(feature = "stretch-signalsmith", not(target_arch = "wasm32")))]
pub(crate) use signalsmith::SignalsmithBackend;
#[cfg(all(feature = "stretch-signalsmith", not(target_arch = "wasm32")))]
pub use signalsmith::SignalsmithElastic;
