use crate::size::{Dim, SizeSpec};

/// A hairline separating two runs of a bar.
#[derive(kithara_derive::Control)]
#[control(size = SizeSpec::new(Dim::Fixed(skin.divider.width), Dim::Fill))]
pub(crate) struct Divider;

#[cfg(any(feature = "iced", feature = "masonry"))]
mod host {
    use super::Divider;
    use crate::{
        atoms::bar::divider::Divider as Face,
        hosts::controls::{Draws, Reading},
        render::Skin,
    };

    impl Draws for Divider {
        type Painter = Face;

        fn data(&self, _read: Reading<'_>) -> Option<()> {
            Some(())
        }

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(skin)
        }
    }
}
