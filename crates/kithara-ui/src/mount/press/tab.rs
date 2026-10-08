use crate::size::{Dim, SizeSpec};

/// A full-width tab heading one page of a panel.
#[derive(kithara_derive::Control)]
#[control(size = SizeSpec::new(Dim::Fill, Dim::Fixed(skin.tab_large.height)), composes_size = false)]
pub(crate) struct Tab;

#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) mod host {
    use bon::Builder;

    use crate::ids::InternId;

    #[derive(Builder)]
    pub(crate) struct Tab {
        pub(crate) label: InternId,
    }

    use crate::{
        atoms::{painter::Labelled, tab::TabLarge},
        hosts::controls::{Draws, Grip, Reading},
        render::{ReadValue, Skin},
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
