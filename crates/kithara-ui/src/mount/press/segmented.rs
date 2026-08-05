use bon::Builder;

use crate::{ids::InternId, mount::Control, size::SizeSpec, skin::SkinDoc};

/// A row of mutually exclusive segments, one of them picked.
#[derive(Builder)]
pub(crate) struct Segmented<'a> {
    pub(crate) items: &'a [InternId],
}

impl Control for Segmented<'_> {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.segmented.size
    }
}
