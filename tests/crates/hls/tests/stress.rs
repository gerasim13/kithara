#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    reason = "integration test crate - unwraps are acceptable in test code"
)]

mod common {
    pub(crate) use kithara_integration_tests::test_defaults;
}
#[path = "../../../tests/common/continuity.rs"]
mod continuity;

#[path = "hls/abr_auto_switch.rs"]
mod abr_auto_switch;
#[path = "hls/abr_mode_switch.rs"]
mod abr_mode_switch;
#[path = "hls/abr_switch_playback.rs"]
mod abr_switch_playback;
#[path = "hls/idle_behavior.rs"]
mod idle_behavior;
#[path = "hls/live_stress_real_stream.rs"]
mod live_stress_real_stream;
#[path = "hls/red_flaky_small_cache_hot_refetch.rs"]
mod red_flaky_small_cache_hot_refetch;
#[path = "hls/red_leak_native_drm_seek_resume.rs"]
mod red_leak_native_drm_seek_resume;
#[path = "hls/startup_no_eager_size_probe_storm.rs"]
mod startup_no_eager_size_probe_storm;
#[path = "hls/stress_seek_abr.rs"]
mod stress_seek_abr;
#[path = "hls/stress_seek_abr_audio.rs"]
mod stress_seek_abr_audio;
#[path = "hls/stress_seek_audio.rs"]
mod stress_seek_audio;
#[path = "hls/stress_seek_lifecycle.rs"]
mod stress_seek_lifecycle;
