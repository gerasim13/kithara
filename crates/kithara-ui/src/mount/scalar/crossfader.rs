use bon::Builder;

use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A horizontal fader centred on its midpoint.
#[derive(Builder)]
pub(crate) struct Crossfader {
    pub(crate) ticks: bool,
}

impl Control for Crossfader {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.crossfader.size
    }
}
