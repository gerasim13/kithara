use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// The window's own caption strip, which also drags the window.
pub(crate) struct TitleBar;

impl Control for TitleBar {
    fn size(&self, _skin: &SkinDoc) -> SizeSpec {
        SizeSpec::FILL
    }
}
