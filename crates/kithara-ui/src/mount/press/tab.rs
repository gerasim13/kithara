use bon::Builder;

use crate::{
    ids::InternId,
    mount::Control,
    size::{Dim, SizeSpec},
    skin::SkinDoc,
};

/// A full-width tab heading one page of a panel.
#[derive(Builder)]
pub(crate) struct Tab {
    pub(crate) label: InternId,
}

impl Control for Tab {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        SizeSpec::new(Dim::Fill, Dim::Fixed(skin.tab_large.height))
    }
}
