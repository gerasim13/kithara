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

#[cfg(feature = "render")]
mod host {
    use super::Select;
    use crate::{
        atoms::design::select::Select as Face,
        compile::CompiledUi,
        render::{ReadValue, Reads, Skin, controls::Draws},
    };

    impl Draws for Select {
        type Painter = Face;

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(skin)
        }

        /// A select shows the word the document wrote; no endpoint moves it.
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
