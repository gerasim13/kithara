use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A toned dot beside a word.
pub(crate) struct StatusDot;

impl Control for StatusDot {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.status_dot.size
    }
}
