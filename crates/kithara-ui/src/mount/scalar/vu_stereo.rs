use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A horizontal pair of level bars with a volume cap.
pub(crate) struct VuStereo;

impl Control for VuStereo {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.vu_stereo.size
    }
}
