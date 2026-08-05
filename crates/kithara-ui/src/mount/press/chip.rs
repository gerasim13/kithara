use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A small labelled toggle that reads as a tag.
pub(crate) struct Chip;

impl Control for Chip {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.chip.size
    }
}
