use bon::Builder;

#[cfg(any(feature = "iced", feature = "masonry"))]
use crate::ids::InternId;

/// One box of a grid, optionally captioned and optionally picked out.
#[derive(Builder, kithara_derive::ViewControl, kithara_derive::Control)]
#[control(size = skin.cell.size)]
#[derive(kithara_derive::NodeControl)]
pub(crate) struct Cell {
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) label: Option<InternId>,
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) highlighted: bool,
}

#[cfg(any(feature = "iced", feature = "masonry"))]
mod host {
    use super::Cell;
    use crate::{
        atoms::{design::cell::Cell as Face, painter::CellData},
        render::{
            Skin,
            controls::{Draws, Reading},
        },
    };

    impl Draws for Cell {
        type Painter = Face;

        fn data(&self, read: Reading<'_>) -> Option<CellData> {
            Some(CellData {
                highlighted: self.highlighted,
                label: self
                    .label
                    .map(|label| read.ctx.ui.resolve(label).to_owned()),
            })
        }

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(skin)
        }
    }
}
