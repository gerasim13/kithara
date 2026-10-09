/// A square switch bound to one boolean endpoint.
#[derive(kithara_derive::Control)]
#[control(size = skin.checkbox.size)]
pub(crate) struct Checkbox;

#[cfg(any(feature = "iced", feature = "masonry"))]
mod host {
    use super::Checkbox;
    use crate::{
        atoms::toggle::Binary,
        hosts::controls::{Draws, Grip, Reading},
        render::{ReadValue, Skin},
    };

    impl Draws for Checkbox {
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
            Binary::checkbox(skin)
        }
    }
}
