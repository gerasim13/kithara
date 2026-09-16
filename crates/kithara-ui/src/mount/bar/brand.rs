/// The wordmark at the head of the global bar.
#[derive(kithara_derive::ViewControl, kithara_derive::Control)]
#[control(size = skin.global_bar.brand_size)]
#[derive(kithara_derive::NodeControl)]
pub(crate) struct Brand;

#[cfg(feature = "render")]
mod host {
    use super::Brand;
    use crate::{
        atoms::bar::brand::Brand as Face,
        render::{
            Skin,
            controls::{Draws, Reading},
        },
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
