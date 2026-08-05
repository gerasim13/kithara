use crate::{
    mount::Control,
    size::{Dim, SizeSpec},
    skin::SkinDoc,
};

/// A hairline separating two runs of a bar.
pub(crate) struct Divider;

impl Control for Divider {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        SizeSpec::new(Dim::Fixed(skin.divider.width), Dim::Fill)
    }
}
