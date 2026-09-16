/// A sliding switch bound to one boolean endpoint.
#[derive(kithara_derive::ViewControl, kithara_derive::Control)]
#[control(size = skin.toggle.size)]
#[derive(kithara_derive::NodeControl)]
pub(crate) struct Toggle;

#[cfg(feature = "render")]
mod host {
    use super::Toggle;
    use crate::{
        atoms::toggle::Binary,
        render::{
            ReadValue, Skin,
            controls::{Draws, Grip, Reading},
        },
    };

    impl Draws for Toggle {
        type Painter = Binary;

        fn data(&self, read: Reading<'_>) -> Option<bool> {
            match read.value {
                Some(ReadValue::Bool(active)) => Some(*active),
                _ => None,
            }
        }

        fn grip(&self, _skin: &Skin, _data: &bool) -> Grip {
            Grip::Press
        }

        fn painter(&self, skin: &Skin) -> Binary {
            Binary::toggle(skin)
        }
    }
}
