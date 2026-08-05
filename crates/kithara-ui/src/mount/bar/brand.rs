use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// The wordmark at the head of the global bar.
pub(crate) struct Brand;

impl Control for Brand {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.global_bar.brand_size
    }
}
