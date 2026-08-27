// A web-audio context runs at whatever rate the browser hands it, and nothing
// on wasm resamples on the way out, so a build without a backend resolves
// `PlaybackResamplerBackend` to `NoResamplerBackend` and every off-rate track
// fails to open.
#[cfg(all(
    target_arch = "wasm32",
    not(any(feature = "resample-rubato", feature = "resample-glide"))
))]
compile_error!("kithara-play: wasm32 build requires `resample-rubato` or `resample-glide`");

#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))
))]
compile_error!("kithara-play: non-wasm build requires `stretch-signalsmith` or `stretch-bungee`");
