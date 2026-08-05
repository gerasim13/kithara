use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// The library tree, with its own search field.
pub(crate) struct Tree;

impl Control for Tree {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.tree.size
    }
}
