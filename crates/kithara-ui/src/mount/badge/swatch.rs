use bon::Builder;

use crate::{ids::InternId, skin::ColorRole};

/// One palette colour, shown with its name.
#[derive(Builder, kithara_derive::ViewControl, kithara_derive::Control)]
#[control(size = skin.swatch.size)]
#[derive(kithara_derive::NodeControl)]
pub(crate) struct Swatch {
    pub(crate) role: ColorRole,
    pub(crate) label: InternId,
}

#[cfg(feature = "render")]
mod host {
    use super::Swatch;
    use crate::{
        atoms::design::swatch::Swatch as Face,
        render::{
            Skin,
            controls::{Draws, Reading},
        },
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
