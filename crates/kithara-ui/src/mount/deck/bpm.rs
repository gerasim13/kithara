use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// The deck's tempo, editable in place.
pub(crate) struct Bpm;

impl Control for Bpm {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.deck.bpm_size
    }
}
