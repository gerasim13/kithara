use crate::{
    mount::Control,
    size::{Dim, SizeSpec},
    skin::SkinDoc,
};

/// A hairline separating two runs of a bar.
pub(crate) struct Divider;

impl Control for Divider {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        SizeSpec::new(Dim::Fixed(skin.divider.width), Dim::Fill)
    }
}

#[cfg(feature = "render")]
mod host {
    use super::Divider;
    use crate::{
        atoms::bar::divider::Divider as Face,
        compile::CompiledUi,
        render::{ReadValue, Reads, Skin, controls::Draws},
    };

    impl Draws for Divider {
        type Painter = Face;

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(skin)
        }

        fn data(
            &self,
            _value: Option<&ReadValue<'_>>,
            _reads: &dyn Reads,
            _ui: &CompiledUi,
        ) -> Option<()> {
            Some(())
        }
    }
}
