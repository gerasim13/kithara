use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// A sliding switch bound to one boolean endpoint.
pub(crate) struct Toggle;

impl Control for Toggle {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.toggle.size
    }
}

#[cfg(feature = "render")]
mod host {
    use super::Toggle;
    use crate::{
        atoms::toggle::Binary,
        compile::CompiledUi,
        render::{
            ReadValue, Skin,
            controls::{Draws, Grip},
        },
    };

    impl Draws for Toggle {
        type Painter = Binary;

        fn painter(&self, skin: &Skin) -> Binary {
            Binary::toggle(skin)
        }

        fn data(&self, value: Option<&ReadValue<'_>>, _ui: &CompiledUi) -> Option<bool> {
            match value {
                Some(ReadValue::Bool(active)) => Some(*active),
                _ => None,
            }
        }

        fn grip(&self) -> Grip {
            Grip::Press
        }
    }
}
