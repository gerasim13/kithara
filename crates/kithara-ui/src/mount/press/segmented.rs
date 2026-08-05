use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A row of mutually exclusive segments, one of them picked.
pub(crate) struct Segmented;

impl Control for Segmented {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.segmented.size
    }
}
