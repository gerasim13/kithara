use crate::session;

/// Current playback position in seconds (read from shared atomics).
pub fn bridge_position_secs() -> f64 {
    session::bridge_position_secs()
}

/// Current media duration in seconds (read from shared atomics).
pub fn bridge_duration_secs() -> f64 {
    session::bridge_duration_secs()
}

/// Whether playback is active (read from shared atomics).
pub fn bridge_is_playing() -> bool {
    session::bridge_is_playing()
}

/// Audio-thread process calls served so far (read from shared atomics).
/// Monotonic; sample twice and read the delta.
#[must_use]
pub fn bridge_process_calls() -> u64 {
    session::bridge_process_calls()
}

/// Underruns the audio thread has recorded (read from shared atomics).
#[must_use]
pub fn bridge_underruns() -> u64 {
    session::bridge_underruns()
}
