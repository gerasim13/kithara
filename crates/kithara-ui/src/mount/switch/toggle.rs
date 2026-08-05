use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A sliding switch bound to one boolean endpoint.
pub(crate) struct Toggle;

impl Control for Toggle {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.toggle.size
    }
}
