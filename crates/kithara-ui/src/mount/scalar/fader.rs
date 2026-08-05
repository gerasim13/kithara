use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A rail and a cap, dragged along the rail.
pub(crate) struct Fader;

impl Control for Fader {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.fader.size
    }
}
