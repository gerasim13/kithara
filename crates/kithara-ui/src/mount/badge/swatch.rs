use bon::Builder;

use crate::{
    ids::InternId,
    mount::Control,
    size::SizeSpec,
    skin::{ColorRole, SkinDoc},
};

/// One palette colour, shown with its name.
#[derive(Builder)]
pub(crate) struct Swatch {
    pub(crate) label: InternId,
    pub(crate) role: ColorRole,
}

impl Control for Swatch {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.swatch.size
    }
}
