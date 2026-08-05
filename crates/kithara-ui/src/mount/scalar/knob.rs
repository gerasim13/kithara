use bon::Builder;

use crate::{ids::InternId, mount::Control, size::SizeSpec, skin::SkinDoc};

/// A rotary control dragged along the vertical axis.
#[derive(Builder)]
pub(crate) struct Knob {
    pub(crate) label: Option<InternId>,
}

impl Control for Knob {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.knob.size
    }
}
