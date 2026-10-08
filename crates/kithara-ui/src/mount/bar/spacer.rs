/// Empty room that pushes its neighbours apart.
#[derive(kithara_derive::Control)]
#[control(size = skin.global_bar.spacer_size)]
pub(crate) struct Spacer;

#[cfg(any(feature = "iced", feature = "masonry"))]
mod host {
    use super::Spacer;
    use crate::{
        atoms::bar::spacer::Spacer as Face,
        hosts::controls::{Draws, Reading},
        render::Skin,
    };

    impl Draws for Spacer {
        type Painter = Face;

        fn data(&self, _read: Reading<'_>) -> Option<()> {
            Some(())
        }

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(skin)
        }
    }
}
