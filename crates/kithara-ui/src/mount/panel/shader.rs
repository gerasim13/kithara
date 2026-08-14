use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A document-owned shader that occupies its declared layout box.
pub(crate) struct Shader;

impl Control for Shader {
    fn size(&self, _skin: &SkinDoc) -> SizeSpec {
        SizeSpec::FILL
    }
}
