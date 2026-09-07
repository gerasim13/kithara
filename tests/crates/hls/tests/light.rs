#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    reason = "integration test crate - unwraps are acceptable in test code"
)]

mod common {
    pub(crate) use kithara_integration_tests::test_defaults;
}

#[path = "hls/aac_he_v2_hls_decode.rs"]
mod aac_he_v2_hls_decode;
#[path = "hls/abr_integration.rs"]
mod abr_integration;
#[path = "hls/basic_playback.rs"]
mod basic_playback;
#[path = "hls/cancel_isolation.rs"]
mod cancel_isolation;
#[path = "hls/config_with_downloader.rs"]
mod config_with_downloader;
#[path = "hls/deferred_abr.rs"]
mod deferred_abr;
#[path = "hls/driver_test.rs"]
mod driver_test;
#[path = "hls/ephemeral.rs"]
mod ephemeral;
#[path = "hls/forward_withheld_segment_busy_spin.rs"]
mod forward_withheld_segment_busy_spin;
#[path = "hls/html_error_body.rs"]
mod html_error_body;
#[path = "hls/html_error_cleanup.rs"]
mod html_error_cleanup;
#[path = "hls/keys_integration.rs"]
mod keys_integration;
#[path = "hls/playlist_integration.rs"]
mod playlist_integration;
#[path = "hls/prefetch_403_fails_open.rs"]
mod prefetch_403_fails_open;
#[path = "hls/probe_not_ready_at_creation.rs"]
mod probe_not_ready_at_creation;
#[path = "hls/red_abr_no_escape_from_stalled_variant.rs"]
mod red_abr_no_escape_from_stalled_variant;
#[path = "hls/red_leak_pattern.rs"]
mod red_leak_pattern;
#[path = "hls/red_leak_peer_handle_cycle.rs"]
mod red_leak_peer_handle_cycle;
#[path = "hls/red_leak_small_cache_seek.rs"]
mod red_leak_small_cache_seek;
#[path = "hls/red_stale_tmp_claim_bricks_segment.rs"]
mod red_stale_tmp_claim_bricks_segment;
#[path = "hls/seek_past_eof.rs"]
mod seek_past_eof;
#[path = "hls/seek_variant_switch_after_eof.rs"]
mod seek_variant_switch_after_eof;
#[path = "hls/segment_boundary_strand.rs"]
mod segment_boundary_strand;
#[path = "hls/smoke_test.rs"]
mod smoke_test;
#[path = "hls/source_seek.rs"]
mod source_seek;
#[path = "hls/sync_reader_hls_test.rs"]
mod sync_reader_hls_test;
#[path = "hls/wait_range_contract.rs"]
mod wait_range_contract;
