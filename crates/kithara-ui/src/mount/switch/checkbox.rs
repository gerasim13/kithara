use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A square switch bound to one boolean endpoint.
pub(crate) struct Checkbox;

impl Control for Checkbox {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.checkbox.size
    }
}
