use bon::Builder;

use crate::{ids::InternId, mount::Control, size::SizeSpec, skin::SkinDoc};

/// One box of a grid, optionally captioned and optionally picked out.
#[derive(Builder)]
pub(crate) struct Cell {
    pub(crate) highlighted: bool,
    pub(crate) label: Option<InternId>,
}

impl Control for Cell {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.cell.size
    }
}

#[cfg(feature = "render")]
mod host {
    use super::Cell;
    use crate::{
        atoms::{design::cell::Cell as Face, painter::CellData},
        compile::CompiledUi,
        render::{ReadValue, Skin, controls::Draws},
    };

    impl Draws for Cell {
        type Painter = Face;

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(skin)
        }

        fn data(&self, _value: Option<&ReadValue<'_>>, ui: &CompiledUi) -> Option<CellData> {
            Some(CellData {
                highlighted: self.highlighted,
                label: self.label.map(|label| ui.resolve(label).to_owned()),
            })
        }
    }
}
