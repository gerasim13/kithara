use bon::Builder;

use crate::ids::InternId;

/// A labelled picker the document opens.
#[derive(Builder, kithara_derive::ViewControl, kithara_derive::Control)]
#[control(size = skin.select.size)]
#[derive(kithara_derive::NodeControl)]
pub(crate) struct Select {
    pub(crate) label: InternId,
}

#[cfg(feature = "render")]
mod host {
    use super::Select;
    use crate::{
        atoms::design::select::Select as Face,
        render::{
            Skin,
            controls::{Draws, Reading},
        },
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
