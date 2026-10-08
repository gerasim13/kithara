/// A labelled picker the document opens.
#[derive(kithara_derive::Control)]
#[control(size = skin.select.size)]
pub(crate) struct Select;

#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) mod host {
    use bon::Builder;

    use crate::ids::InternId;

    #[derive(Builder)]
    pub(crate) struct Select {
        pub(crate) label: InternId,
    }

    use crate::{
        atoms::design::select::Select as Face,
        hosts::controls::{Draws, Reading},
        render::Skin,
    };

    impl Draws for Select {
        type Painter = Face;

        /// A select shows the word the document wrote; no endpoint moves it.
        fn data(&self, read: Reading<'_>) -> Option<String> {
            Some(read.ctx.ui.resolve(self.label).to_owned())
        }

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(skin)
        }
    }
}
