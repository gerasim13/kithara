use bon::Builder;

use crate::{
    expand::Binding,
    ids::InternId,
    mount::Control,
    size::{Dim, SizeSpec},
    skin::SkinDoc,
};

/// The strip under the tree that names the scope in view.
#[derive(Builder)]
pub(crate) struct ContextBar<'a> {
    pub(crate) scope: Option<&'a Binding>,
    pub(crate) scope_items: &'a [InternId],
}

impl Control for ContextBar<'_> {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        SizeSpec::new(Dim::Fill, Dim::Fixed(skin.tree.context_height))
    }
}
