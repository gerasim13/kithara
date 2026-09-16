/// Empty room that pushes its neighbours apart.
#[derive(kithara_derive::ViewControl, kithara_derive::Control)]
#[control(size = skin.global_bar.spacer_size)]
#[derive(kithara_derive::NodeControl)]
pub(crate) struct Spacer;

#[cfg(feature = "render")]
mod host {
    use super::Spacer;
    use crate::{
        atoms::bar::spacer::Spacer as Face,
        render::{
            Skin,
            controls::{Draws, Reading},
        },
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
