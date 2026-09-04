/// Whether this target includes a native Warp rendering backend.
#[must_use]
pub const fn supports_playback_rate() -> bool {
    cfg!(all(
        not(target_arch = "wasm32"),
        any(feature = "stretch-signalsmith", feature = "stretch-bungee")
    ))
}
