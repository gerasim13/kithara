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

#[cfg(feature = "render")]
mod host {
    use super::StatusDot;
    use crate::{
        atoms::design::status_dot::StatusDot as Face,
        compile::CompiledUi,
        render::{ReadValue, Reads, Skin, controls::Draws},
    };

    impl Draws for StatusDot {
        type Painter = Face;

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(self.tone, skin)
        }

        fn data(
            &self,
            _value: Option<&ReadValue<'_>>,
            _reads: &dyn Reads,
            ui: &CompiledUi,
        ) -> Option<String> {
            Some(ui.resolve(self.label).to_owned())
        }
    }
}
