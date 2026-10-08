/// The global bar's own button, which opens the settings surface.
#[derive(kithara_derive::Control)]
#[control(size = skin.global_bar.settings_size)]
pub(crate) struct Settings;

#[cfg(any(feature = "iced", feature = "masonry"))]
mod host {
    use super::Settings;
    use crate::{
        atoms::bar::settings::Settings as Face,
        hosts::{
            controls::{Draws, Grip, Reading},
            icons::Mark,
        },
        module::IconName,
        render::Skin,
    };

    impl Draws for Settings {
        type Painter = Face;

        /// The gear is the button: art that cannot be read leaves an empty box
        /// rather than a frame with a hole in it.
        fn data(&self, _read: Reading<'_>) -> Option<Mark> {
            IconName::Gear.mark()
        }

        fn grip(&self, _skin: &Skin, _data: &Mark) -> Grip {
            Grip::Press
        }

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(skin)
        }
    }
}
