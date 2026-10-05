use crate::{draw::Rect, render::Skin};

#[derive(Clone, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct Search {
    #[field(get, vis = "pub(crate)")]
    query: String,
    #[field(get, vis = "pub(crate)")]
    skin: Skin,
}

impl Search {
    pub(crate) fn new(query: &str, skin: &Skin) -> Self {
        Self {
            query: query.to_owned(),
            skin: skin.clone(),
        }
    }
}

pub(crate) fn input_bounds(bounds: Rect, skin: &Skin) -> Rect {
    Rect {
        h: skin.tree.search_height.min(bounds.h.max(0.0)),
        w: (bounds.w - skin.tree.search_icon_width - 1.0).max(0.0),
        x: bounds.x + skin.tree.search_icon_width + 1.0,
        y: bounds.y,
    }
}
