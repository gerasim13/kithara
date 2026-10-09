/// The wordmark at the head of the global bar.
#[derive(kithara_derive::Control)]
#[control(size = skin.global_bar.brand_size)]
pub(crate) struct Brand;

#[cfg(any(feature = "iced", feature = "masonry"))]
mod host {
    use super::Brand;
    use crate::{
        atoms::bar::brand::Brand as Face,
        hosts::controls::{Draws, Reading},
        render::Skin,
    };

    impl Draws for Brand {
        type Painter = Face;

        fn data(&self, _read: Reading<'_>) -> Option<()> {
            Some(())
        }

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(skin)
        }
    }
}
