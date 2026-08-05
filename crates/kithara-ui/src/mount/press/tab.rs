use crate::{
    mount::Control,
    size::{Dim, SizeSpec},
    skin::SkinDoc,
};

/// A full-width tab heading one page of a panel.
pub(crate) struct Tab;

impl Control for Tab {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        SizeSpec::new(Dim::Fill, Dim::Fixed(skin.tab_large.height))
    }
}
