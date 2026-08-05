use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A horizontal fader centred on its midpoint.
pub(crate) struct Crossfader;

impl Control for Crossfader {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.crossfader.size
    }
}
