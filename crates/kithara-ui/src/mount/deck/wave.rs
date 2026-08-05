use bon::Builder;

use crate::{
    expand::Binding, ids::InternId, module::WaveStyle, mount::Control, size::SizeSpec,
    skin::SkinDoc,
};

/// The track's waveform, zoomed and scrubbed.
#[derive(Builder)]
pub(crate) struct Wave<'a> {
    pub(crate) badge: Option<InternId>,
    pub(crate) style: WaveStyle,
    pub(crate) zoom: Option<&'a Binding>,
}

impl Control for Wave<'_> {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.wave.size
    }
}
