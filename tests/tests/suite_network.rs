#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    reason = "integration test crate — unwraps are acceptable in test code"
)]
//! Every test that reaches a host this workspace does not own.
//!
//! Two kinds live here. Some need the public internet plus credentials baked
//! at build time (`KITHARA_DRM_PROD_*`, the `cdn-hls-slicer.zvuk.com` tracks);
//! the rest need the corporate VPN on top, because `*.zvq.me` resolves only
//! from inside it. Neither is available to CI, and neither ever will be: the
//! runners are deliberately outside that network.
//!
//! The suite compiles only with the `network` feature, so a default `just test`
//! never builds it and no lane can run it by accident. Run it by hand from a
//! machine that has the network and the credentials:
//!
//! ```text
//! just test run --lane=network
//! ```
//!
//! A test that stops needing a remote host belongs in `suite_light.rs` beside
//! its fixtures, not here.

#[cfg(not(target_arch = "wasm32"))]
mod kithara_play {
    mod live_remote_network;
    mod silvercomet_seek_hang;
}

#[cfg(not(target_arch = "wasm32"))]
mod kithara_queue {
    mod source_helper;
    use source_helper::app_track_source;

    mod cold_seek_cpal;
    mod false_eof_rapid_scrub;
    mod real_playlist;
    mod zvuk_drm_trace;
    mod zvuk_prod_aac_to_flac_switch;
    mod zvuk_prod_drm_e2e;
    mod zvuk_prod_flac_swallow;
    mod zvuk_stage_drm_e2e;
    mod zvuk_stage_seed_brute_force;

    mod user_simulation {
        mod prod_network;
    }
}
