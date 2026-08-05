use bon::Builder;

use crate::{ids::InternId, module::Tone, mount::Control, size::SizeSpec, skin::SkinDoc};

/// A caption with a value beside it, toned by the document.
#[derive(Builder)]
pub(crate) struct Readout {
    pub(crate) framed: bool,
    pub(crate) label: Option<InternId>,
    pub(crate) tone: Tone,
}

impl Control for Readout {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.readout.size
    }
}
