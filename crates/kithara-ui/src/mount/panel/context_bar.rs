use crate::{
    mount::Control,
    size::{Dim, SizeSpec},
    skin::SkinDoc,
};

/// The strip under the tree that names the scope in view.
pub(crate) struct ContextBar;

impl Control for ContextBar {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        SizeSpec::new(Dim::Fill, Dim::Fixed(skin.tree.context_height))
    }
}
