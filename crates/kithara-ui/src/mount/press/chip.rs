/// A small labelled toggle that reads as a tag.
#[derive(kithara_derive::Control)]
#[control(size = skin.chip.size)]
pub(crate) struct Chip;

#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) mod host {
    use bon::Builder;

    use crate::{ids::InternId, module::ChipStyle};

    #[derive(Builder)]
    pub(crate) struct Chip {
        pub(crate) style: ChipStyle,
        pub(crate) label: InternId,
    }

    use crate::{
        atoms::{chip::Chip as Face, painter::Labelled},
        hosts::controls::{Draws, Grip, Reading},
        render::{ReadValue, Skin},
    };

    impl Draws for Chip {
        type Painter = Face;

        /// A chip carries a word the document wrote, so it shows it whether or
        /// not an endpoint has said which way it is set — unlike a switch,
        /// which is nothing but its state.
        fn data(&self, read: Reading<'_>) -> Option<Labelled> {
            Some(Labelled {
                active: matches!(read.value, Some(ReadValue::Bool(true))),
                label: read.ctx.ui.resolve(self.label).to_owned(),
            })
        }

        fn grip(&self, _skin: &Skin, _data: &Labelled) -> Grip {
            Grip::Press
        }

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(self.style, skin)
        }
    }
}
