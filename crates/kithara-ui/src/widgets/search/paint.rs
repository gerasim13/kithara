use super::input_bounds;
use crate::{
    atoms::{icon::mark::Marked, text_input::TextInputPaint},
    draw::{DrawListBuilder, Rect},
    engine::TextInputSnapshot,
    module::IconName,
    render::Skin,
    shaping::TextContext,
};

#[cfg(feature = "masonry")]
pub(crate) fn paint(
    list: &mut DrawListBuilder,
    text: &mut TextContext,
    bounds: Rect,
    query: &str,
    skin: &Skin,
    snapshot: &TextInputSnapshot,
) {
    let input = TextInputPaint::with_context(query.into(), skin, text);
    paint_shaped(list, text, bounds, &input, skin, snapshot);
}

/// Paints the search face around a query `input` already shaped.
pub(crate) fn paint_shaped(
    list: &mut DrawListBuilder,
    text: &mut TextContext,
    bounds: Rect,
    input: &TextInputPaint<'_>,
    skin: &Skin,
    snapshot: &TextInputSnapshot,
) {
    let search = Rect {
        h: skin.tree.search_height.min(bounds.h.max(0.0)),
        ..bounds
    };
    let icon = Rect {
        w: skin.tree.search_icon_width.min(search.w.max(0.0)),
        ..search
    };
    list.fill_rect(search, skin.rgba(skin.tree.search_divider));
    list.fill_rect(icon, skin.rgba(skin.tree.search_background));
    if let Some(mark) = IconName::Search.mark() {
        Marked::new(mark, skin.tree.search_icon_size).centred(
            list,
            text,
            icon,
            skin.rgba(skin.tree.search_icon_color),
        );
    }
    input.paint(list, snapshot, input_bounds(bounds, skin));
}
