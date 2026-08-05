use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// Empty room that pushes its neighbours apart.
pub(crate) struct Spacer;

impl Control for Spacer {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.global_bar.spacer_size
    }
}
