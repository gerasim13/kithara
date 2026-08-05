use bon::Builder;

use crate::{ids::InternId, module::FaderStyle, mount::Control, size::SizeSpec, skin::SkinDoc};

/// A rail and a cap, dragged along the rail.
#[derive(Builder)]
pub(crate) struct Fader {
    pub(crate) label: Option<InternId>,
    pub(crate) style: FaderStyle,
}

impl Control for Fader {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.fader.size
    }
}
