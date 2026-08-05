use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A rotary control dragged along the vertical axis.
pub(crate) struct Knob;

impl Control for Knob {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.knob.size
    }
}
