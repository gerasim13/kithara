/// One palette colour, shown with its name.
#[derive(kithara_derive::Control)]
#[control(size = skin.swatch.size)]
pub(crate) struct Swatch;

#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) mod host {
    use bon::Builder;

    use crate::{ids::InternId, skin::ColorRole};

    #[derive(Builder)]
    pub(crate) struct Swatch {
        pub(crate) role: ColorRole,
        pub(crate) label: InternId,
    }

    use crate::{
        atoms::design::swatch::Swatch as Face,
        hosts::controls::{Draws, Reading},
        render::Skin,
    };

    impl Draws for Swatch {
        type Painter = Face;

        fn data(&self, read: Reading<'_>) -> Option<String> {
            Some(read.ctx.ui.resolve(self.label).to_owned())
        }

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(self.role, skin)
        }
    }
}
