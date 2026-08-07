use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A horizontal bar filled from the left to show one fraction.
pub(crate) struct Meter;

impl Control for Meter {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.meter.size
    }
}

#[cfg(feature = "render")]
mod host {
    use num_traits::cast::AsPrimitive;

    use super::Meter;
    use crate::{
        atoms::design::meter::Meter as Face,
        compile::CompiledUi,
        render::{ReadValue, Reads, Skin, controls::Draws},
    };

    impl Draws for Meter {
        type Painter = Face;

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(skin)
        }

        /// An unbound meter is an empty track rather than an empty box: a level
        /// nobody reports is a level of nothing.
        fn data(
            &self,
            value: Option<&ReadValue<'_>>,
            _reads: &dyn Reads,
            _ui: &CompiledUi,
        ) -> Option<f32> {
            Some(match value {
                Some(ReadValue::Scalar(level)) => level.clamp(0.0, 1.0).as_(),
                _ => 0.0,
            })
        }
    }
}
