use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A vertical pair of level bars with a volume cap.
pub(crate) struct VuVertical;

impl Control for VuVertical {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.vu_vertical.size
    }
}
