use bon::Builder;

use crate::{
    ids::InternId,
    size::{Dim, SizeSpec},
};

/// A full-width tab heading one page of a panel.
#[derive(Builder, kithara_derive::ViewControl, kithara_derive::Control)]
#[control(size = SizeSpec::new(Dim::Fill, Dim::Fixed(skin.tab_large.height)), composes_size = false)]
#[derive(kithara_derive::NodeControl)]
pub(crate) struct Tab {
    pub(crate) label: InternId,
}

#[cfg(feature = "render")]
mod host {
    use super::Tab;
    use crate::{
        atoms::{painter::Labelled, tab::TabLarge},
        render::{
            ReadValue, Skin,
            controls::{Draws, Grip, Reading},
        },
    };

    impl Draws for Tab {
        type Painter = TabLarge;

        /// A tab heads a page, so one whose endpoint has not said whether its
        /// page is the current one draws nothing rather than a tab at rest.
        fn data(&self, read: Reading<'_>) -> Option<Labelled> {
            let Some(ReadValue::Bool(active)) = read.value else {
                return None;
            };
            Some(Labelled {
                active: *active,
                label: read.ctx.ui.resolve(self.label).to_owned(),
            })
        }

        fn grip(&self, _skin: &Skin, _data: &Labelled) -> Grip {
            Grip::Press
        }

        fn painter(&self, skin: &Skin) -> TabLarge {
            TabLarge::new(skin)
        }
    }
}
