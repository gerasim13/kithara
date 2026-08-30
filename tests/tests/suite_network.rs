#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    reason = "integration test crate — unwraps are acceptable in test code"
)]
//! The tests that reach a host this workspace does not own, and that a runner
//! can reach: the public zvuk CDN and silvercomet.
//!
//! The CDN is auth-gated, so this suite only says something about the code
//! where `KITHARA_DRM_PROD_*` are in the build environment — kithara-app's
//! build script bakes them, so they belong to the build rather than to the test
//! process. Without them the tests compile and fail on the key request, which
//! is why this is a lane of its own rather than a gate job.
//!
//! What a runner cannot reach lives in `suite_network_manual`: the corporate
//! hosts, and the tests that open a real output device. The split is by what a
//! machine can serve, not by what the test is about — kept together, the whole
//! set stayed off CI because part of it could not go there.
//!
//! ```text
//! just test run --lane=network
//! ```

pub use kithara_integration_tests::bufpool_ext;

#[cfg(not(target_arch = "wasm32"))]
mod kithara_play {
    mod silvercomet_seek_hang;
}

#[cfg(not(target_arch = "wasm32"))]
mod kithara_queue {
    mod source_helper;
    use source_helper::app_track_source;

    mod false_eof_rapid_scrub;
    mod real_playlist;
    mod zvuk_prod_aac_to_flac_switch;
    mod zvuk_prod_drm_e2e;
    mod zvuk_prod_flac_swallow;
    mod zvuk_stage_seed_brute_force;

    mod user_simulation {
        mod prod_network;
    }
}
