use std::num::NonZeroU32;

use firewheel::FirewheelContext;
use firewheel_web_audio::WebAudioBackend;

use super::client::WebSessionState;
use crate::HostOwner;

pub(super) fn init_bridge_state() {
    todo!(
        "Publish browser playback and diagnostics from the new deck snapshots, not PlaybackShared (spec §5.7)"
    )
}
pub(super) fn reset_bridge_state() {
    todo!("Retire the browser owner snapshot on shutdown (spec §4.1)")
}

pub(crate) fn bridge_position_secs() -> f64 {
    todo!("Read media position from the canonical deck snapshot (spec §5.7)")
}
pub(crate) fn bridge_duration_secs() -> f64 {
    todo!("Read duration from the canonical deck snapshot (spec §5.7)")
}
pub(crate) fn bridge_process_calls() -> u64 {
    todo!("Read mixer process counts from the canonical deck snapshot (spec §5.7)")
}
pub(crate) fn bridge_underruns() -> u64 {
    todo!("Read mixer underruns from the canonical deck snapshot (spec §5.7)")
}
pub(crate) fn bridge_is_playing() -> bool {
    todo!("Read playback state from the canonical deck snapshot (spec §5.7)")
}

pub(crate) fn warm_up_audio<S, O: HostOwner<S>>(
    _state: &WebSessionState<O>,
) -> Result<(), crate::session::SessionError> {
    todo!(
        "Warm the browser backend through its canonical owner without lending concrete SessionState to the facade (spec §4.2)"
    )
}

pub(super) fn start_stream_web_audio(
    ctx: &mut FirewheelContext,
    sample_rate: u32,
) -> Result<WebAudioBackend, String> {
    WebAudioBackend::new(
        ctx,
        firewheel_web_audio::WebAudioConfig {
            sample_rate: NonZeroU32::new(sample_rate),
            request_input: false,
        },
    )
    .map_err(|error| error.to_string())
}
