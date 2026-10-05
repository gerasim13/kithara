use kithara_stretch::{BackendCapabilities, StretchKind};

/// Whether an available Warp rendering backend changes playback rate.
#[must_use]
pub const fn supports_playback_rate() -> bool {
    let backends = StretchKind::all();
    let mut index = 0;
    while index < backends.len() {
        if backends[index]
            .capabilities()
            .contains(BackendCapabilities::RATE)
        {
            return true;
        }
        index += 1;
    }
    false
}
