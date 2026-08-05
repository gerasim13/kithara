use bon::Builder;

use crate::{ids::InternId, module::Tone, mount::Control, size::SizeSpec, skin::SkinDoc};

/// A toned dot beside a word.
#[derive(Builder)]
pub(crate) struct StatusDot {
    pub(crate) label: InternId,
    pub(crate) tone: Tone,
}

impl Control for StatusDot {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.status_dot.size
    }
}
