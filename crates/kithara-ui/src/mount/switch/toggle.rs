/// A sliding switch bound to one boolean endpoint.
#[derive(kithara_derive::Control)]
#[control(size = skin.toggle.size)]
pub(crate) struct Toggle;

#[cfg(any(feature = "iced", feature = "masonry"))]
mod host {
    use super::Toggle;
    use crate::{
        atoms::toggle::Binary,
        hosts::controls::{Draws, Grip, Reading},
        render::{ReadValue, Skin},
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
