use bon::Builder;

use crate::{
    ids::InternId,
    module::IconName,
    mount::Control,
    size::{Dim, SizeSpec},
    skin::SkinDoc,
};

/// One row of the navigation rail: an icon, a word, and a selected state.
#[derive(Builder)]
pub(crate) struct NavItem {
    pub(crate) icon: IconName,
    pub(crate) label: InternId,
}

impl Control for NavItem {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        SizeSpec::new(Dim::Fill, Dim::Fixed(skin.nav.item_height))
    }
}
