#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    reason = "integration test crate — unwraps are acceptable in test code"
)]

mod common;

#[cfg(not(target_arch = "wasm32"))]
#[path = "common/gapless.rs"]
mod gapless_common;

#[cfg(not(target_arch = "wasm32"))]
mod kithara_audio {
    mod alloc_free_hotpath;
}

#[cfg(not(target_arch = "wasm32"))]
#[path = "kithara_queue/sync_behavioral_matrix.rs"]
mod sync_behavioral_matrix;
#[cfg(not(target_arch = "wasm32"))]
#[path = "kithara_queue/sync_latency.rs"]
mod sync_latency;
#[cfg(not(target_arch = "wasm32"))]
#[path = "kithara_queue/sync_library.rs"]
mod sync_library;
#[cfg(not(target_arch = "wasm32"))]
#[path = "kithara_queue/sync_media.rs"]
mod sync_media;
#[cfg(not(target_arch = "wasm32"))]
#[path = "kithara_queue/sync_rt.rs"]
mod sync_rt;

mod kithara_decode {
    #[cfg(not(target_arch = "wasm32"))]
    mod fixture_integration;
    #[cfg(not(target_arch = "wasm32"))]
    mod gapless_encoding_parity;
    #[cfg(not(target_arch = "wasm32"))]
    mod gapless_parity;
    #[cfg(not(target_arch = "wasm32"))]
    mod hls_abr_variant_switch;
    mod stress_timeline;

    #[cfg(not(target_arch = "wasm32"))]
    mod stress_seek_random;
}

// The rest of this suite drives the local test server, the filesystem, and
// several players at once. The browser has none of that; only
// `kithara_ffi_web` is meant to run there.
#[cfg(not(target_arch = "wasm32"))]
mod kithara_file {
    mod live_stress_real_mp3;
}

#[cfg(not(target_arch = "wasm32"))]
mod kithara_hls {
    mod deferred_abr_debug;
    mod drm_stream_integrity;
    mod stress_chunk_integrity;
    mod stress_seek_random;
}

mod kithara_ffi_web;

#[cfg(not(target_arch = "wasm32"))]
mod multi_instance;
