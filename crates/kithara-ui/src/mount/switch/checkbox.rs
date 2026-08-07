use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A square switch bound to one boolean endpoint.
pub(crate) struct Checkbox;

impl Control for Checkbox {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.checkbox.size
    }
}

#[cfg(feature = "render")]
mod host {
    use super::Checkbox;
    use crate::{
        atoms::toggle::Binary,
        compile::CompiledUi,
        render::{
            ReadValue, Reads, Skin,
            controls::{Draws, Grip},
        },
    };

    impl Draws for Checkbox {
        type Painter = Binary;

        fn painter(&self, skin: &Skin) -> Binary {
            Binary::checkbox(skin)
        }

        fn data(
            &self,
            value: Option<&ReadValue<'_>>,
            _reads: &dyn Reads,
            _ui: &CompiledUi,
        ) -> Option<bool> {
            match value {
                Some(ReadValue::Bool(active)) => Some(*active),
                _ => None,
            }
        }

        fn grip(&self, _skin: &Skin, _data: &bool) -> Grip {
            Grip::Press
        }
    }
}
