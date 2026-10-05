#![forbid(unsafe_code)]

use kithara_test_dylib as _;

mod common {
    pub(crate) use kithara_integration_tests::test_defaults;
}

mod abr_auto_switch;
mod abr_mode_switch;
mod abr_switch_playback;
mod idle_behavior;
mod live_stress_real_stream;
mod phase_continuity;
mod red_flaky_small_cache_hot_refetch;
mod red_leak_native_drm_seek_resume;
mod saw_chunk;
mod startup_no_eager_size_probe_storm;
mod stress_seek_abr;
mod stress_seek_abr_audio;
mod stress_seek_audio;
mod stress_seek_lifecycle;
