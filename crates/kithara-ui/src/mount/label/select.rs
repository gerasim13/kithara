use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A labelled picker the document opens.
pub(crate) struct Select;

impl Control for Select {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.select.size
    }
}
