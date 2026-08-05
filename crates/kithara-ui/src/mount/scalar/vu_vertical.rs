use bon::Builder;

use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A vertical pair of level bars with a volume cap.
#[derive(Builder)]
pub(crate) struct VuVertical {
    pub(crate) ticks: bool,
}

impl Control for VuVertical {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.vu_vertical.size
    }
}
