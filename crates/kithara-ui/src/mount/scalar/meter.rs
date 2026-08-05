use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A horizontal bar filled from the left to show one fraction.
pub(crate) struct Meter;

impl Control for Meter {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.meter.size
    }
}
