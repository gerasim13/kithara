use bon::Builder;

use crate::{ids::InternId, mount::Control, size::SizeSpec, skin::SkinDoc};

/// The deck's tempo, editable in place.
#[derive(Builder)]
pub(crate) struct Bpm {
    pub(crate) placeholder: Option<InternId>,
}

impl Control for Bpm {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.deck.bpm_size
    }
}
