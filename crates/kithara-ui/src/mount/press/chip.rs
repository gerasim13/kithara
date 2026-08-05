use bon::Builder;

use crate::{ids::InternId, module::ChipStyle, mount::Control, size::SizeSpec, skin::SkinDoc};

/// A small labelled toggle that reads as a tag.
#[derive(Builder)]
pub(crate) struct Chip {
    pub(crate) label: InternId,
    pub(crate) style: ChipStyle,
}

impl Control for Chip {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.chip.size
    }
}
