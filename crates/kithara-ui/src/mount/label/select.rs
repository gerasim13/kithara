use bon::Builder;

use crate::{ids::InternId, mount::Control, size::SizeSpec, skin::SkinDoc};

/// A labelled picker the document opens.
#[derive(Builder)]
pub(crate) struct Select {
    pub(crate) label: InternId,
}

impl Control for Select {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.select.size
    }
}
