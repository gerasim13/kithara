use crate::{
    mount::Control,
    size::{Dim, SizeSpec},
    skin::SkinDoc,
};

/// One row of the navigation rail: an icon, a word, and a selected state.
pub(crate) struct NavItem;

impl Control for NavItem {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        SizeSpec::new(Dim::Fill, Dim::Fixed(skin.nav.item_height))
    }
}
