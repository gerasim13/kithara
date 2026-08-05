use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A caption with a value beside it, toned by the document.
pub(crate) struct Readout;

impl Control for Readout {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.readout.size
    }
}
