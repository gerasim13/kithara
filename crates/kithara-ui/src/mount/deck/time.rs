use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// The deck's position and what is left of the track.
pub(crate) struct Time;

impl Control for Time {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.deck.time_size
    }
}
