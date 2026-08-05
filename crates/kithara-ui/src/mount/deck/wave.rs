use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// The track's waveform, zoomed and scrubbed.
pub(crate) struct Wave;

impl Control for Wave {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.wave.size
    }
}
