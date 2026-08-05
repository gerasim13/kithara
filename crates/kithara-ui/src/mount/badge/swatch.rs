use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// One palette colour, shown with its name.
pub(crate) struct Swatch;

impl Control for Swatch {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.swatch.size
    }
}
