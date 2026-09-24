#[cfg(all(
    target_arch = "wasm32",
    not(any(feature = "resample-rubato", feature = "resample-glide"))
))]
compile_error!("kithara-play: wasm32 build requires `resample-rubato` or `resample-glide`");
